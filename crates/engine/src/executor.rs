use crate::context::{RunContext, StepOutput, Verdict};
use crate::control::Control;
use crate::manifest::{Manifest, OnUnmet, RunMode, Step, StepKind, Verifier, Verify};
use crate::protocol::{Command, Event, GateKind, LoopEndReason, RunStatus, StepMetrics, StepStatus};
use crate::runner::claude::ClaudeRunner;
use crate::runner::codex::CodexRunner;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;

pub struct RunnerBins {
    pub claude: String,
    pub codex: String,
}

enum StepDecision {
    Retry,
    Skip,
    Abort,
}

pub struct Executor {
    manifest: Manifest,
    ctx: RunContext,
    claude: ClaudeRunner,
    codex: CodexRunner,
    control: Arc<Control>,
    events: Sender<Event>,
    commands: Receiver<Command>,
}

impl Executor {
    /// 返回 Result 的构造器(review-2 §E finding #12 修正):库 API 不该在构造器
    /// 内 panic,SDK 嵌入时整线程崩。CLI / Tauri 已显式 validate,这里再 validate
    /// 一次是第二道兜底 — 返 Err 让 caller 决定如何上报(GUI 弹错误 / CLI 退出码)。
    pub fn try_new(
        manifest: Manifest,
        bins: RunnerBins,
        control: Arc<Control>,
        events: Sender<Event>,
        commands: Receiver<Command>,
    ) -> Result<Self, crate::error::EngineError> {
        manifest.validate()?;
        let mut ctx = RunContext::new(manifest.target.clone());
        ctx.set_budget(manifest.budget_usd);
        Ok(Self {
            claude: ClaudeRunner::new(bins.claude),
            codex: CodexRunner::new(bins.codex),
            manifest,
            ctx,
            control,
            events,
            commands,
        })
    }

    /// 兼容旧 callsite 的薄 wrapper:`try_new(...).expect(...)`。
    /// CLI/Tauri 主路径已 validate,此 expect 实际不会触发;测试路径 panic 同旧行为。
    pub fn new(
        manifest: Manifest,
        bins: RunnerBins,
        control: Arc<Control>,
        events: Sender<Event>,
        commands: Receiver<Command>,
    ) -> Self {
        Self::try_new(manifest, bins, control, events, commands)
            .expect("Manifest 必须先通过 validate() 才能交给 Executor")
    }

    pub fn run(&mut self) -> RunStatus {
        let _ = self.events.send(Event::RunStarted {
            name: self.manifest.name.clone(),
            target: self.manifest.target.display().to_string(),
        });
        // 隔离 worktree:开启则在跑任何 step 前建好,把 cwd 指向它。失败 fail-closed 终止,
        // 绝不退回 target 原地跑(否则隔离形同虚设)。
        if self.manifest.worktree {
            match crate::worktree::create(&self.manifest.target, &self.manifest.name) {
                Ok(wt) => {
                    let _ = self.events.send(Event::WorktreeReady {
                        path: wt.path.display().to_string(),
                        branch: wt.branch,
                    });
                    self.ctx.cwd = wt.path;
                }
                Err(e) => {
                    // 取内层 message,避免 "worktree 创建失败: worktree error: …" 双前缀
                    let error = match e {
                        crate::error::EngineError::Worktree(m) => m,
                        other => other.to_string(),
                    };
                    let _ = self.events.send(Event::WorktreeFailed { error });
                    let _ = self.events.send(Event::RunFinished {
                        status: RunStatus::Failed,
                    });
                    return RunStatus::Failed;
                }
            }
        }
        let steps = std::mem::take(&mut self.manifest.steps);
        let gated = matches!(self.manifest.mode, RunMode::Step);
        for step in &steps {
            if self.run_step(step, gated).is_err() {
                // run_step 的 Err 来自:Abort 路径(失败已走决策 gate)/ Control aborted / over_budget。
                // budget 与 Control aborted 都按 Aborted 上报(受外部约束停止,不是 step 工作失败);
                // 单一状态源 = ctx(无独立 over_budget 字段),避免"flag 与 ctx 派生量"漂移。
                let status = if self.ctx.is_over_budget() || self.control.is_aborted() {
                    RunStatus::Aborted
                } else {
                    RunStatus::Failed
                };
                let _ = self.events.send(Event::RunFinished {
                    status: status.clone(),
                });
                return status;
            }
        }
        let _ = self.events.send(Event::RunFinished {
            status: RunStatus::Success,
        });
        RunStatus::Success
    }

    fn run_step(&mut self, step: &Step, gated: bool) -> Result<(), ()> {
        if self.control.is_aborted() {
            return Err(());
        }

        // Loop 自身不门控(body 子步骤逐个门控);Human 自身已有 gate,不重复。
        if gated && !matches!(step.kind, StepKind::Loop { .. } | StepKind::Human { .. }) {
            let _ = self.events.send(Event::StepAwaitingGate {
                step_id: step.id.clone(),
                suggestion: format!("即将执行 step '{}'", step.id),
                expects_artifact: false,
                gate_kind: GateKind::Step,
            });
            match self.commands.recv() {
                Ok(Command::ApproveGate { .. }) => {}
                Ok(Command::SkipStep { .. }) => {
                    self.emit_skipped(&step.id);
                    return Ok(());
                }
                _ => {
                    // 与 decision_gate 同形:step 门控的 Abort 路径也翻 abort 标志,
                    // 让 run() 顶层分类 RunStatus::Aborted(review §A finding #13)。
                    self.control.request_abort();
                    self.fail(&step.id, "aborted".into());
                    return Err(());
                }
            }
        }

        // Loop 自身不发 StepStarted —— 它没有对应的 StepFinished,宿主按 step_id 去重渲染时
        // 会把它显示成永久 "运行中" 的幽灵步骤(GUI 控制台见过)。loop 生命周期由
        // LoopIteration / LoopConverged / LoopMaxReached 表达,body 子步骤各自发 Started/Finished。
        let kind_name = match &step.kind {
            StepKind::Claude { .. } => Some("claude"),
            StepKind::Codex { .. } => Some("codex"),
            StepKind::Human { .. } => Some("human"),
            StepKind::Acp { .. } => Some("acp"),
            StepKind::Loop { .. } => None,
        };
        if let Some(kind) = kind_name {
            let _ = self.events.send(Event::StepStarted {
                step_id: step.id.clone(),
                kind: kind.into(),
            });
        }

        match &step.kind {
            StepKind::Codex { action, path, base, prompt } => {
                // 标识符类字段(base 分支名 / 文件路径)走 interpolate_identifier:
                // 模板用 {{xxx.artifact}} 动态解析时,LLM artifact 常带前后空白 / 多行 /
                // 末尾换行(尤其指令"只输出 X"也未必严格遵守)。引擎统一 trim 取首行非空,
                // 空 → None 让下游 fail-loud。prompt 是自由文本不归一。
                let path_i = self.interpolate_identifier(path.as_deref());
                let base_i = self.interpolate_identifier(base.as_deref());
                let prompt_i = prompt.as_ref().map(|p| self.ctx.interpolate(p));
                let mut on_line = self.progress_sink(&step.id);
                loop {
                    let res = self.codex.review(
                        action,
                        path_i.as_deref(),
                        base_i.as_deref(),
                        prompt_i.as_deref(),
                        Some(self.control.as_ref()),
                        &mut on_line,
                        &self.ctx.cwd,
                    );
                    match res {
                        Ok(out) => {
                            let summary = format!("verdict={:?}", out.verdict);
                            // 先 charge,再 check_budget(StepFailed 携带 cumulative,与
                            // claude/acp 路径同形)。codex 单次 step 没有累积概念,cumulative
                            // = 单次 out.metrics(实际目前 None,等 codex CLI 升级)。
                            self.charge(&out.metrics);
                            self.check_budget(&step.id, &out.metrics)?;
                            self.ctx.record(&step.id, StepOutput {
                                findings: Some(out.findings),
                                verdict: Some(out.verdict),
                                ..Default::default()
                            });
                            self.finish(&step.id, summary, out.metrics);
                            return Ok(());
                        }
                        Err(e) => match self.handle_failure(&step.id, e.to_string()) {
                            StepDecision::Retry => continue,
                            StepDecision::Skip => {
                                self.emit_skipped(&step.id);
                                return Ok(());
                            }
                            StepDecision::Abort => return Err(()),
                        },
                    }
                }
            }
            StepKind::Claude { prompt, skill, verify } => {
                let mut on_line = self.progress_sink(&step.id);
                let mut attempt = 0u32; // 校验重试计数(与失败重试独立)
                let mut feedback: Option<String> = None;
                // 累积该 step 内所有 attempt + verifier 的 metrics(review-2 §B finding #2/#7)。
                // finish 拿到的是 cumulative 而非末次 attempt,审计 / GUI 总 cost 不再低估。
                let mut step_metrics: Option<StepMetrics> = None;
                loop {
                    let mut p = self.ctx.interpolate(prompt);
                    if let Some(f) = &feedback {
                        p.push_str(&format!("\n\n上一轮校验反馈:\n{f}\n请据此修正后重做。"));
                    }
                    let out = match self.claude.run(
                        &p,
                        skill.as_deref(),
                        Some(self.control.as_ref()),
                        &mut on_line,
                        &self.ctx.cwd,
                        false, // 干活步骤可写
                    ) {
                        Ok(out) => out,
                        Err(e) => match self.handle_failure(&step.id, e.to_string()) {
                            StepDecision::Retry => continue,
                            StepDecision::Skip => {
                                self.emit_skipped(&step.id);
                                return Ok(());
                            }
                            StepDecision::Abort => return Err(()),
                        },
                    };
                    let metrics = out.metrics;
                    let answer = out.answer;
                    // 每次 attempt 都 charge —— verify-retry 中段不再丢 cost(本 PR 主修)。
                    // 先 sum 出 cumulative 再 check_budget:budget 触发的 StepFailed 携带
                    // 该 step 至今的全部花费,audit 不漏统计(review §A finding #4)。
                    self.charge(&metrics);
                    step_metrics = Self::sum_metrics(step_metrics, metrics);
                    self.check_budget(&step.id, &step_metrics)?;
                    self.ctx.record(&step.id, StepOutput {
                        artifact: Some(answer.clone()),
                        ..Default::default()
                    });
                    // 用户裁决(2026-06-26):每轮 fix 也要看详情。把 claude answer 文本
                    // 追发到 progress,UI 展开 step 输出能看到本轮干活的最终回答 / 修复说明。
                    // 截断 30 行兜底防长 answer 撑爆面板。
                    emit_answer_preview(&answer, &mut on_line);

                    // 无校验门 → 退出码即完成(原行为)
                    let v = match verify {
                        None => {
                            self.finish(&step.id, "done".into(), step_metrics);
                            return Ok(());
                        }
                        Some(v) => v,
                    };
                    on_line("校验中…", None);
                    let (verdict, findings, verifier_metrics) = self.verify_once(v, &mut on_line);
                    // verifier 自身的 cost 也入账(spec §3.1:verifier 钱也要进 budget)。
                    // 同样 charge → sum → check 顺序,确保 budget StepFailed 拿到包含
                    // verifier 这一笔的 cumulative。
                    self.charge(&verifier_metrics);
                    step_metrics = Self::sum_metrics(step_metrics, verifier_metrics);
                    self.check_budget(&step.id, &step_metrics)?;
                    // 暴露 verifier findings 供下游 {{<id>.findings}} 引用
                    self.ctx.record(&step.id, StepOutput {
                        artifact: Some(answer.clone()),
                        findings: Some(findings.clone()),
                        ..Default::default()
                    });
                    if matches!(verdict, Verdict::Clean) {
                        on_line("校验通过", None);
                        self.finish(&step.id, "done · 已校验".into(), step_metrics);
                        return Ok(());
                    }
                    // 未达成:还有重试预算就带反馈重跑
                    if attempt < v.max_retries {
                        attempt += 1;
                        on_line(&format!("校验未通过,第 {attempt} 次重试"), None);
                        feedback = if v.feedback { Some(findings) } else { None };
                        continue;
                    }
                    // 重试耗尽 → 升级策略
                    match v.on_unmet {
                        OnUnmet::Continue => {
                            self.finish(&step.id, "done · 未达标(continue)".into(), step_metrics);
                            return Ok(());
                        }
                        OnUnmet::Fail => {
                            // cost 已经在每次 attempt + verifier 后 charge 过,此处不再补 charge
                            // (否则重复)。但要把 step_metrics 带进 StepFailed,让 audit
                            // 看到 verify-retry 累积花费 — review §A finding #4 治本(前 PR
                            // 这条路径是「known trade-off」直接丢 cost,与 budget 触发的 fail
                            // 路径同形,本次统一)。
                            self.fail_with_metrics(
                                &step.id,
                                format!("校验未通过(已重试 {} 次)", v.max_retries),
                                step_metrics.clone(),
                            );
                            return Err(());
                        }
                        OnUnmet::Gate => {
                            let suggestion = format!(
                                "校验未通过(重试 {} 次仍未达标),选择 重试 / 跳过 / 中止\n{findings}",
                                v.max_retries
                            );
                            match self.decision_gate(&step.id, suggestion) {
                                StepDecision::Retry => {
                                    attempt = 0; // 人工再批一次,给新预算
                                    feedback = if v.feedback { Some(findings) } else { None };
                                    continue;
                                }
                                StepDecision::Skip => {
                                    self.emit_skipped(&step.id);
                                    return Ok(());
                                }
                                StepDecision::Abort => return Err(()),
                            }
                        }
                    }
                }
            }
            StepKind::Human { instruction, expects, value } => {
                let instr = self.ctx.interpolate(instruction);
                self.run_human(step, &instr, expects.is_some(), value.as_deref())
            }
            StepKind::Acp { agent, command, prompt } => {
                // review §A finding #7:ACP step 不走自动 retry loop。理由:ACP runner
                // 当前 metrics 永远 None(F1),如果遇到 empty-answer / 配错 agent 一类
                // fail-loud 失败,decision_gate 的 Retry 选项在 budget_usd=None 时不被
                // budget 兜底,容易让用户连点 Retry 无限烧 LLM 钱。
                //
                // 与 claude(有 verify-retry 语义)/ codex(单次 review)不同:ACP 是
                // 「跑一次拿 answer」的通用接入层,没有 verify 概念,失败重试本应由
                // 用户手动重启 run(顺便确认是否调整 budget),不在自动决策门内重试。
                // Skip / Abort 仍可走。
                let mut on_line = self.progress_sink(&step.id);
                let p = self.ctx.interpolate(prompt);
                let runner = crate::runner::acp::AcpRunner::new(crate::runner::acp::AcpConfig {
                    agent: agent.clone(),
                    command: command.clone(),
                });
                match runner.run(&p, Some(self.control.as_ref()), &mut on_line, &self.ctx.cwd) {
                    Ok(out) => {
                        // 同 codex/claude 路径:charge 单次 → check_budget 带 cumulative。
                        // ACP 单次 step 无累积,cumulative = 单次。
                        self.charge(&out.metrics);
                        self.check_budget(&step.id, &out.metrics)?;
                        self.ctx.record(&step.id, StepOutput {
                            artifact: Some(out.answer.clone()),
                            ..Default::default()
                        });
                        self.finish(&step.id, "done · acp".into(), out.metrics);
                        Ok(())
                    }
                    Err(e) => {
                        // 失败直接 emit StepFailed + Err 退 step,不走 decision_gate。
                        // 触发 abort 标志让 run() 顶层分类落 Aborted("ACP 失败,人工
                        // 重启 run"),与「budget=None 时的隐式重试 LLM 烧钱」彻底脱钩。
                        self.fail(&step.id, e.to_string());
                        self.control.request_abort();
                        Err(())
                    }
                }
            }
            StepKind::Loop { until, max, body } => self.run_loop(&step.id, until, *max, body, gated),
        }
    }

    /// 发一个决策 gate(重试/跳过/中止)并阻塞等宿主指令。已中止则直接 Abort 不弹门。
    /// 失败重试与校验未达成共用这条通道。
    ///
    /// review §A finding #13:用户经决策门选 Abort 时,必须同步翻 Control::request_abort,
    /// 否则 run() 顶层分类只看 control.is_aborted() / over_budget → 用户主动 Abort 会被
    /// 误分类为 Failed("引擎失败"),Tauri 宿主当前用 Command::Abort+request_abort 配对
    /// 才避开这条,但引擎库 API 不该依赖 host 的对齐 — 这里 fail-loud 翻 abort 标志,
    /// 与 Tauri 路径同构,SDK 嵌入方零负担。
    fn decision_gate(&self, step_id: &str, suggestion: String) -> StepDecision {
        if self.control.is_aborted() {
            return StepDecision::Abort;
        }
        let _ = self.events.send(Event::StepAwaitingGate {
            step_id: step_id.to_string(),
            suggestion,
            expects_artifact: false,
            gate_kind: GateKind::Decision,
        });
        match self.commands.recv() {
            Ok(Command::ApproveGate { .. }) => StepDecision::Retry,
            Ok(Command::SkipStep { .. }) => StepDecision::Skip,
            _ => {
                // 决策门 Abort(显式 Command::Abort / 信道关闭) → 翻 abort 标志,让
                // run() 顶层分类自然走 RunStatus::Aborted("用户中止"),与 Tauri 路径
                // 对齐;子进程已退出,kill_current 是 no-op,无副作用。
                self.control.request_abort();
                StepDecision::Abort
            }
        }
    }

    /// step 失败(进程非零)的处理:发 StepFailed 后走决策 gate。
    /// runner Err 路径没有可信的累积成本(spawn 失败 / 超时等),metrics = None;
    /// budget 触发 / OnUnmet::Fail 的有 metrics 路径走 check_budget / fail_with_metrics。
    fn handle_failure(&mut self, step_id: &str, err: String) -> StepDecision {
        if self.control.is_aborted() {
            return StepDecision::Abort;
        }
        self.fail(step_id, err);
        self.decision_gate(step_id, "step 失败,选择 重试 / 跳过 / 中止".into())
    }

    /// 跑一次校验门,返回 (verdict, findings, metrics)。metrics 让上层把 verifier
    /// 自身的 cost 也入账(spec §3.1 主目标:verifier 的钱也要进 budget)。verifier
    /// 自身失败/不可解析一律 fail-closed 为 ChangesRequested(绝不静默判过)。
    /// Command verifier 无 cost 概念,metrics 永远 None。
    fn verify_once(
        &self,
        v: &Verify,
        on_line: &mut dyn FnMut(&str, Option<u32>),
    ) -> (Verdict, String, Option<StepMetrics>) {
        match v.by {
            Verifier::Codex => {
                // action 必有(validate 已保证);防御性兜底 fail-closed。
                let action = match &v.action {
                    Some(a) => a,
                    None => return (Verdict::ChangesRequested, "verify codex 缺 action".into(), None),
                };
                // 标识符类字段统一 interpolate_identifier(同 StepKind::Codex 路径)。
                let path_i = self.interpolate_identifier(v.path.as_deref());
                let base_i = self.interpolate_identifier(v.base.as_deref());
                let prompt_i = v.prompt.as_ref().map(|p| self.ctx.interpolate(p));
                match self.codex.review(
                    action,
                    path_i.as_deref(),
                    base_i.as_deref(),
                    prompt_i.as_deref(),
                    Some(self.control.as_ref()),
                    on_line,
                    &self.ctx.cwd,
                ) {
                    Ok(out) => (out.verdict, out.findings, out.metrics),
                    Err(e) => (Verdict::ChangesRequested, format!("校验执行失败: {e}"), None),
                }
            }
            Verifier::Claude => {
                let q = self.ctx.interpolate(v.prompt.as_deref().unwrap_or(""));
                let judge = format!(
                    "{q}\n\n请只读判定上述目标是否达成,不要修改任何文件。\
                     在回复最后单独一行输出 `VERDICT: pass`(达成)或 `VERDICT: fail`(未达成)。"
                );
                match self.claude.run(
                    &judge,
                    v.skill.as_deref(),
                    Some(self.control.as_ref()),
                    on_line,
                    &self.ctx.cwd,
                    true, // verifier 只读(plan 模式)
                ) {
                    Ok(out) => (parse_verdict(&out.answer), out.answer, out.metrics),
                    Err(e) => {
                        // review §A finding #3:Verifier::Claude Err 路径 metrics=None。
                        // claude CLI 在 Err 之前可能已经烧了部分 token(超时 / 非零退出
                        // 都在传输 stream-json 末尾 result 行之前发生 → metrics 拿不到);
                        // 这一笔 cost 不会进 ctx.cost_so_far_usd,budget 触发不了。
                        // 显式 eprintln warn 让用户知道 budget guard 在此 attempt 失效
                        // (CLI/Tauri 未装 tracing_subscriber,用 stderr 直发保证可见)。
                        eprintln!(
                            "[agentpipe] WARN: Verifier::Claude 执行失败,本次 attempt 的 \
                             cost 未上报、不会扣 budget — 错误: {e}"
                        );
                        (Verdict::ChangesRequested, format!("校验执行失败: {e}"), None)
                    }
                }
            }
            Verifier::Command => {
                let cmd = match &v.command {
                    Some(c) if !c.trim().is_empty() => c.as_str(),
                    _ => return (Verdict::ChangesRequested, "verify command 缺 command 字段".into(), None),
                };
                on_line("校验命令…", None);
                let (verdict, findings) = command_verdict(
                    cmd,
                    &self.ctx.cwd,
                    self.control.as_ref(),
                    &mut |l| on_line(l, None),
                );
                (verdict, findings, None)
            }
        }
    }

    fn run_human(
        &mut self,
        step: &Step,
        instruction: &str,
        expects_artifact: bool,
        seed: Option<&str>,
    ) -> Result<(), ()> {
        // 启动时预置了人工输入 → 直接记录为产物,跳过 gate(不阻塞 recv)。
        // 插值后为空视同未预置,回退到正常 gate(防止 GUI 漏填仍能人工补)。
        if let Some(raw) = seed {
            let val = self.ctx.interpolate(raw);
            if !val.trim().is_empty() {
                self.ctx.record(
                    &step.id,
                    StepOutput {
                        artifact: Some(val),
                        ..Default::default()
                    },
                );
                self.finish(&step.id, "approved (preset)".into(), None);
                return Ok(());
            }
        }
        let _ = self.events.send(Event::StepAwaitingGate {
            step_id: step.id.clone(),
            suggestion: instruction.to_string(),
            expects_artifact,
            gate_kind: GateKind::Human,
        });
        match self.commands.recv() {
            Ok(Command::ApproveGate { artifact, .. }) => {
                self.ctx.record(&step.id, StepOutput {
                    artifact,
                    ..Default::default()
                });
                self.finish(&step.id, "approved".into(), None);
                Ok(())
            }
            Ok(Command::SkipStep { .. }) => {
                self.emit_skipped(&step.id);
                Ok(())
            }
            _ => {
                // Human 门 abort 与 step / decision 门同形:翻 abort 标志让顶层分类
                // 落到 RunStatus::Aborted(review §A finding #13)。
                self.control.request_abort();
                self.fail(&step.id, "aborted".into());
                Err(())
            }
        }
    }

    fn run_loop(&mut self, loop_id: &str, until: &str, max: u32, body: &[Step], gated: bool) -> Result<(), ()> {
        for n in 1..=max {
            if self.control.is_aborted() {
                // 控制中止:emit LoopMaxReached{reason: Aborted}让 UI 渲染区分于「自然耗
                // 尽 max」(review §A finding #15)。前 PR 共用 MaxReached 让用户看到
                // 「hit max 0, still not clean」语义噪音 — reason 明确分流。
                let _ = self.events.send(Event::LoopMaxReached {
                    loop_id: loop_id.into(),
                    max: n.saturating_sub(1),
                    reason: LoopEndReason::Aborted,
                });
                return Err(());
            }
            let _ = self.events.send(Event::LoopIteration {
                loop_id: loop_id.into(),
                iteration: n,
            });
            for sub in body {
                if self.run_step(sub, gated).is_err() {
                    // sub-step 失败 Err 透传:reason=SubStepFailed,与控制中止 / 自然 max
                    // 三态分明。UI/CLI 各自按 reason 出不同文案,不再「都说 hit max」。
                    let _ = self.events.send(Event::LoopMaxReached {
                        loop_id: loop_id.into(),
                        max: n,
                        reason: LoopEndReason::SubStepFailed,
                    });
                    return Err(());
                }
            }
            if self.eval_until(until, body) {
                let _ = self.events.send(Event::LoopConverged {
                    loop_id: loop_id.into(),
                    iterations: n,
                });
                return Ok(());
            }
        }
        let _ = self.events.send(Event::LoopMaxReached {
            loop_id: loop_id.into(),
            max,
            reason: LoopEndReason::MaxReached,
        });
        Ok(())
    }

    /// 目前只支持 codex-clean:找 body 里最后一个 codex step 的 verdict。
    fn eval_until(&self, until: &str, body: &[Step]) -> bool {
        if until != "codex-clean" {
            return false;
        }
        for sub in body.iter().rev() {
            if matches!(sub.kind, StepKind::Codex { .. }) {
                if let Some(out) = self.ctx.get(&sub.id) {
                    return matches!(out.verdict, Some(Verdict::Clean));
                }
            }
        }
        false // 没找到 codex step → fail-closed 不收敛
    }

    // (helper 见文件末 emit_answer_preview)

    /// 标识符类字段(分支名 / 文件路径 / 其他 git ref / shell-safe 名)的插值归一:
    /// `{{xxx.artifact}}` 解析结果常带前后空白 / 多行 / 末尾换行(LLM 即使被指令
    /// 「只输出 X」也未必严格遵守)。取首行非空 trim 后字符串;空 → None 让下游
    /// 路径 fail-loud(避免空串当合法 ref / 含 `\n` 的 `cwd.join` 静默失败)。
    /// 与 prompt / instruction 这类自由文本字段区分,后者保留原样不归一。
    fn interpolate_identifier(&self, raw: Option<&str>) -> Option<String> {
        let r = raw?;
        let interpolated = self.ctx.interpolate(r);
        interpolated
            .lines()
            .map(str::trim)
            .find(|s| !s.is_empty())
            .map(|s| s.to_string())
    }

    /// 生成一个把 CLI 进度(行 + 可选模型轮次)转成 StepProgress 事件的回调
    /// (克隆 events,不借 self)。claude 传实际轮次,codex 传 None。
    fn progress_sink(&self, step_id: &str) -> impl FnMut(&str, Option<u32>) {
        let events = self.events.clone();
        let sid = step_id.to_string();
        move |line: &str, round: Option<u32>| {
            let _ = events.send(Event::StepProgress {
                step_id: sid.clone(),
                line: line.to_string(),
                round,
            });
        }
    }

    /// 纯事件发射 — emit StepFinished{Done},不累加 cost、不判 budget。
    /// budget 由 `charge` + `check_budget` 在每个 spawn 点单独负责(review §A finding
    /// #4 之后由 charge_and_check 拆分:check_budget 要拿到 cumulative metrics 才能
    /// 把它带进 over-budget StepFailed,让 audit 不漏统计),避免 cost 只在末次 attempt
    /// finish 时入账(verify-retry / Fail 路径漏统计)。
    fn finish(&self, step_id: &str, summary: String, metrics: Option<StepMetrics>) {
        let _ = self.events.send(Event::StepFinished {
            step_id: step_id.to_string(),
            status: StepStatus::Done,
            summary,
            metrics,
        });
    }

    /// 累加两份 metrics 的 num_turns / duration_ms / cost_usd;任一为 None 直接返另一个。
    /// 用于 Claude verify-retry 路径:把所有 attempt + verifier 的 metrics 在 step 内
    /// 累积,让 StepFinished 携带的 metrics 反映该 step 的**全部**花费(review-2 §B 修
    /// audit/UI cost 漏统计:之前 finish 只传末次 attempt 一份,verifier 与早期 attempt
    /// 全部不进 emit,与 ctx.cost_so_far_usd 的实际累计不一致)。
    fn sum_metrics(a: Option<StepMetrics>, b: Option<StepMetrics>) -> Option<StepMetrics> {
        match (a, b) {
            (None, x) | (x, None) => x,
            (Some(a), Some(b)) => Some(StepMetrics {
                num_turns: a.num_turns + b.num_turns,
                duration_ms: a.duration_ms + b.duration_ms,
                cost_usd: a.cost_usd + b.cost_usd,
            }),
        }
    }

    /// 累加一次 spawn 的成本(单次 delta),不判 budget。专门做 cost 累加这一件事,
    /// 与 check_budget 拆开 — review §A finding #4 后:budget 触发的 StepFailed 必须
    /// 携带 step 内 cumulative metrics(让 audit 不漏统计),check_budget 需要拿到
    /// cumulative 才能 emit,charge 单次 + cumulative sum 必须语义分离。
    fn charge(&mut self, metrics: &Option<StepMetrics>) {
        let cost = metrics.as_ref().map(|m| m.cost_usd).unwrap_or(0.0);
        self.ctx.add_cost(cost);
    }

    /// 判 budget;超额 emit StepFailed{metrics: step_cumulative}并 Err(())。caller 在
    /// `charge` + `sum_metrics` 累计后立即调用,确保 emit 出去的失败事件携带该 step
    /// 至今的完整花费(audit::aggregate_cost 能正确合计 — 修补旧版「budget 砍 step,
    /// audit 显示 $0」漂移,review §A finding #4)。
    fn check_budget(
        &self,
        step_id: &str,
        step_metrics: &Option<StepMetrics>,
    ) -> Result<(), ()> {
        if self.ctx.is_over_budget() {
            let spent = self.ctx.cost_so_far_usd();
            let cap = self
                .ctx
                .budget_usd()
                .expect("is_over_budget guarantees Some");
            let _ = self.events.send(Event::StepFailed {
                step_id: step_id.to_string(),
                error: format!(
                    "超出 USD budget (累计 ${spent:.4} > 上限 ${cap:.4} during step '{step_id}')"
                ),
                metrics: step_metrics.clone(),
            });
            return Err(());
        }
        Ok(())
    }

    fn emit_skipped(&self, step_id: &str) {
        let _ = self.events.send(Event::StepFinished {
            step_id: step_id.to_string(),
            status: StepStatus::Skipped,
            summary: "skipped".into(),
            metrics: None,
        });
    }

    fn fail(&self, step_id: &str, error: String) {
        self.fail_with_metrics(step_id, error, None);
    }

    /// fail 的带 cumulative metrics 变体,供 OnUnmet::Fail 等已累积花费的路径
    /// 把 step 内 attempt + verifier 的 cost 一并送给 audit。review §A finding #4。
    fn fail_with_metrics(&self, step_id: &str, error: String, metrics: Option<StepMetrics>) {
        let _ = self.events.send(Event::StepFailed {
            step_id: step_id.to_string(),
            error,
            metrics,
        });
    }
}

/// 把 claude attempt 的 answer 文本追发到 progress sink:让 UI 展开 step 输出能看到
/// 本轮干活的最终回答 / 修复说明,而不只是「调用 Bash / 调用 Edit / 思考中」类标签。
/// 用户裁决(2026-06-26):每轮 review/fix 要看详情。
///
/// 截断 30 行兜底:实现 step 经常输出几千字,完整 emit 会让 UI 渲染抖动 + 内存压力。
/// 用户想看完整 answer 走 audit NDJSON / 下游 step 的 `{{xxx.artifact}}` 插值。
fn emit_answer_preview(answer: &str, on_line: &mut dyn FnMut(&str, Option<u32>)) {
    let trimmed = answer.trim();
    if trimmed.is_empty() {
        on_line("─── 完成 · (无文本输出) ───", None);
        return;
    }
    on_line("─── 完成 · 输出 ───", None);
    let total = trimmed.lines().count();
    const PREVIEW_LINES: usize = 30;
    for line in trimmed.lines().take(PREVIEW_LINES) {
        on_line(line, None);
    }
    if total > PREVIEW_LINES {
        on_line(
            &format!("…(还有 {} 行,完整内容见审计 NDJSON / 下游插值)", total - PREVIEW_LINES),
            None,
        );
    }
}

/// 从 claude verifier 的回复里解析 `VERDICT: pass|fail`(末行优先)。
/// 找不到 / 非 pass 一律 fail-closed 为 ChangesRequested(绝不静默判过)。
fn parse_verdict(answer: &str) -> Verdict {
    for line in answer.lines().rev() {
        if let Some(rest) = line.trim().strip_prefix("VERDICT:") {
            return if rest.trim().eq_ignore_ascii_case("pass") {
                Verdict::Clean
            } else {
                Verdict::ChangesRequested
            };
        }
    }
    Verdict::ChangesRequested
}

/// 用 shell 命令做校验门:exit 0 = 达成(Clean),否则 ChangesRequested(findings = 输出尾部)。
/// 复用 runner::run_command(进程组 / pgid 登记 / abort kill 已就绪);`2>&1` 把 stderr 并进捕获。
/// spawn / IO 失败、被信号杀死一律 fail-closed 为 ChangesRequested。
fn command_verdict(
    cmd: &str,
    cwd: &std::path::Path,
    control: &crate::control::Control,
    on_line: &mut dyn FnMut(&str),
) -> (Verdict, String) {
    let shell_cmd = format!("{cmd} 2>&1");
    match crate::runner::run_command(
        "sh",
        &["-c".into(), shell_cmd],
        cwd,
        None,
        None,
        Some(control),
        on_line,
    ) {
        Ok((_, true)) => (Verdict::Clean, String::new()),
        Ok((out, false)) => (Verdict::ChangesRequested, tail(&out, 4096)),
        Err(e) => (Verdict::ChangesRequested, format!("校验执行失败: {e}")),
    }
}

/// 取字符串末 max_bytes 字节,向后对齐到 char 边界(不切碎 UTF-8)。
fn tail(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut start = s.len() - max_bytes;
    while start < s.len() && !s.is_char_boundary(start) {
        start += 1;
    }
    s[start..].to_string()
}

#[cfg(test)]
mod tests {
    use super::{command_verdict, parse_verdict, tail};
    use crate::context::Verdict;
    use crate::control::Control;
    use std::path::Path;

    #[test]
    fn parse_verdict_reads_sentinel() {
        assert!(matches!(parse_verdict("分析…\nVERDICT: pass"), Verdict::Clean));
        assert!(matches!(parse_verdict("VERDICT: PASS"), Verdict::Clean));
        assert!(matches!(parse_verdict("VERDICT: fail\n收尾"), Verdict::ChangesRequested));
    }

    #[test]
    fn parse_verdict_fail_closed_without_sentinel() {
        assert!(matches!(parse_verdict("没有判定行"), Verdict::ChangesRequested));
        assert!(matches!(parse_verdict(""), Verdict::ChangesRequested));
    }

    #[test]
    fn command_verdict_exit_zero_is_clean() {
        let ctrl = Control::default();
        let (v, f) = command_verdict("exit 0", Path::new("."), &ctrl, &mut |_l| {});
        assert!(matches!(v, Verdict::Clean));
        assert!(f.is_empty());
    }

    #[test]
    fn command_verdict_nonzero_is_changes_with_output() {
        let ctrl = Control::default();
        let (v, f) = command_verdict("echo boom; exit 1", Path::new("."), &ctrl, &mut |_l| {});
        assert!(matches!(v, Verdict::ChangesRequested));
        assert!(f.contains("boom"));
    }

    #[test]
    fn command_verdict_missing_binary_fail_closed() {
        let ctrl = Control::default();
        let (v, _f) = command_verdict("definitely_not_a_real_cmd_xyz", Path::new("."), &ctrl, &mut |_l| {});
        assert!(matches!(v, Verdict::ChangesRequested));
    }

    #[test]
    fn command_verdict_spawn_failure_fail_closed() {
        let ctrl = Control::default();
        // 不存在的 cwd → spawn 失败 → run_command 返回 Err → 走 "校验执行失败" 分支
        let (v, f) = command_verdict("echo hi", Path::new("/nonexistent/agentpipe/xyz"), &ctrl, &mut |_l| {});
        assert!(matches!(v, Verdict::ChangesRequested));
        assert!(f.contains("校验执行失败"), "Err 分支文案不对: {f}");
    }

    #[test]
    fn tail_keeps_suffix_on_char_boundary() {
        assert_eq!(tail("hello", 100), "hello");
        assert_eq!(tail("abcdefgh", 3), "fgh");
        // 多字节字符不被切碎
        let s = "藏字符串末尾"; // 每个汉字 3 字节
        let t = tail(s, 4); // 4 字节落在某汉字中间,应向后对齐到边界
        assert!(s.ends_with(&t));
    }
}

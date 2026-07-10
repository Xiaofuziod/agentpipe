use super::{run_command, stderr_hint, CommandOutput};
use crate::control::Control;
use crate::context::{Severity, Verdict};
use crate::error::EngineError;
use crate::manifest::CodexAction;
use crate::protocol::{FindingItem, ReviewResult, StepMetrics};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Once, OnceLock};

static OUT_SEQ: AtomicU64 = AtomicU64::new(0);

/// 一次性 warn:codex CLI 当前不在 stdout 输出 token usage,Verifier::Codex 不上报
/// metrics,budget_usd 对 codex 验证步骤形同虚设(review §A finding #2)。
static WARN_CODEX_COST_BYPASS: Once = Once::new();

/// review prompt 共用尾巴:要求 reviewer 为每个 finding 给具体可执行 suggestion。
/// 两处 prompt 共用,避免漂移(spec §3.2)。**自带前导空格 + 句号收尾**,与调用方
/// prompt 拼接时不产生双标点;**强约束「必须提供」** 保留模型动机(review-2 §A
/// finding #6:避免「可选」措辞让 LLM 走最省路径默认略过)。
const SUGGESTION_HINT: &str = " 每个 finding **必须**提供 suggestion 字段:具体可执行的修改建议(例:'第 42 行 nil 检查改成 if let Some(x) = y { ... }' / '把 unwrap 改为 ? 传播');无具体建议时填 \"N/A\"。";

/// codex review 单次墙钟上限(秒)。挂死 / provider 失联时到点 kill 整组,
/// review() 返回 Err 走 step 失败决策门,绝不冻住整个 run。可经
/// `AGENTPIPE_CODEX_TIMEOUT_SECS` 覆盖(>0 生效)。
/// 默认 1200s(20min):大 MR review 读大 diff + 推理本就慢,留足空间;
/// 仍远低于无超时时观测到的 6.5h 冻死。
const DEFAULT_CODEX_TIMEOUT_SECS: u64 = 1200;

pub struct CodexRunner {
    bin: String,
    timeout_secs: u64,
}

#[derive(Deserialize)]
struct RawReview {
    verdict: String,
    #[serde(default)]
    findings: Vec<RawFinding>,
}

#[derive(Deserialize)]
struct RawFinding {
    // 全部字段 required:与 REVIEW_SCHEMA `required: [...]` 对齐(OpenAI strict mode
    // 要求所有 properties 都 required,additionalProperties:false)。任一字段缺失即整条
    // 解析失败,走 fallback ChangesRequested,不静默渲染空串 / 默认值喂下游 fixer。
    //
    // review §A finding #11:旧版 suggestion 留 `#[serde(default)]` 与 schema required
    // 矛盾 —— 生产 OpenAI 拒老 codex 二进制(无 suggestion 字段)在 schema 层、serde
    // 反而 default 空串通过,两层语义对不上让 reader 困惑且 legacy guard 实际死代码。
    // 现统一 fail-loud:旧 codex 二进制必须升级,与 spec §3.2 文档明示「min codex
    // version」对齐。
    severity: String,
    file: String,
    line: i64,
    summary: String,
    /// 具体修改建议(spec §3.2),提升下游 fixer 的可操作性。
    suggestion: String,
}

impl CodexRunner {
    pub fn new(bin: String) -> Self {
        let timeout_secs = super::timeout_secs_from_env(
            "AGENTPIPE_CODEX_TIMEOUT_SECS",
            DEFAULT_CODEX_TIMEOUT_SECS,
        );
        Self { bin, timeout_secs }
    }

    /// 显式指定超时(秒),供测试注入小值;生产走 `new` 的默认 / env。
    pub fn with_timeout(bin: String, timeout_secs: u64) -> Self {
        Self { bin, timeout_secs }
    }

    /// 返回 ReviewResult。解析失败一律 fail-closed 为 ChangesRequested。
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    pub fn review(
        &self,
        action: &CodexAction,
        doc_path: Option<&str>,
        base: Option<&str>,
        ask_prompt: Option<&str>,
        vet: bool,
        // review-mr 的 git pathspec 排除项;其余 action 忽略。空 = 审全部(默认)。
        exclude: &[String],
        control: Option<&Control>,
        on_progress: &mut dyn FnMut(&str, Option<u32>),
        cwd: &Path,
    ) -> Result<ReviewResult, EngineError> {
        // 一次性显式告警:codex review 不上报 cost,budget_usd 对 codex 验证 step 无效。
        // review §A finding #2:让用户可观测 budget guard 在此通道处于 inactive,
        // 避免"配了 budget 仍被烧光"的反认知体验。等 codex CLI 升级出 usage 后填实。
        // 用 eprintln 同 acp.rs:CLI/Tauri 未装 tracing_subscriber,tracing 会被吞。
        WARN_CODEX_COST_BYPASS.call_once(|| {
            eprintln!(
                "[agentpipe] WARN: codex review 当前不上报 token cost(codex CLI \
                 暂无 usage 输出),codex step / Verifier::Codex 不计入 budget_usd \
                 — 如需 budget 兜底请用 Verifier::Claude 或等 codex CLI 升级。"
            );
        });

        let (out_file, out_str) = out_file_for("codex");
        let schema = write_schema()?;

        // 全部走通用 `codex exec` + 严格 --output-schema 拿结构化 verdict。
        // 注:codex v0.139.0 只把最终结构化结果打到 stdout、不写 -o(--output-last-message);
        // v0.144.0 实测两边都写。故下方以 stdout 为主、-o 为 fallback,兼容两代
        // (`codex exec review` 子命令的 -o 写散文,弃用)。
        // review-doc 把文档内容经 stdin 喂给 codex(spec 7.2);其余 action 无 stdin。
        // vet 开启时同步构造 VetScope:核验是全新 codex exec 进程,主审查的范围
        // (base 分支 / 文档内容)必须重申,否则核验无从判断 finding 是否属于本次
        // 审查对象(review finding #5)。
        let (args, stdin, vet_scope): (Vec<String>, Option<String>, Option<VetScope>) = match action {
            CodexAction::ReviewMr => {
                // base 必须由 caller 提供(写死或模板用 {{...}} 动态解析,见 templates/);
                // 不再有 fallback "dev" — 写死 "dev" 是「review-mr 默认审 dev」的隐式假设,
                // 在 main / master 仓库直接出错且报「ref `dev` 无法解析」让用户困惑。
                // 现在 None → fail-loud 明示「未提供 base 字段」,引导用户用 gh pr view
                // 动态填(see templates/mr-review-loop.yaml 的 base-detect step)。
                let b = match base {
                    Some(b) => b,
                    None => {
                        return Err(EngineError::Cli(
                            "review-mr 必须提供 base 字段(目标分支名)。模板里通常用 \
                             `base: \"{{base-detect.artifact}}\"` 让 claude step 跑 \
                             `gh pr view <MR-URL> --json baseRefName -q .baseRefName` 动态\
                             解析,或直接写死为该 MR 的真实目标分支(如 main / master)。"
                                .into(),
                        ));
                    }
                };
                // base ref 预检:review-mr 审的是 `git diff {b}...HEAD`。若 {b} 在目标仓库
                // 不可解析(分支名配错 / 仓库未 fetch 到该分支),codex 跑 diff 必然失败,
                // 只能把"没法审"返回成 changes_requested。引擎若信任该 verdict 喂回 loop,
                // until:codex-clean 永不满足 → loop 空转烧钱到 max(活锁,已观测)。
                // 这里 fail-loud 返回 Err → executor 走 step 失败决策门(暂停/中止),不静默放过。
                if !base_ref_resolvable(cwd, b) {
                    return Err(EngineError::Cli(format!(
                        "审查基线 ref `{b}` 在目标仓库无法解析(`git diff {b}...HEAD` 会报 unknown revision)。\
                         请确认 task 的 review.base 是该 MR 的真实目标分支(如 main/master),\
                         且目标仓库已 fetch 到该分支。"
                    )));
                }
                // exclude 与 diff 命令同源:vet 是全新 codex 进程,它判定"finding 是否属于
                // 本次审查范围"用的必须是同一条命令。两处漂移会让 vet 拿全量范围去核验
                // 一份缩小范围的 findings,反之亦然。
                validate_exclude(exclude)?;
                let diff_cmd = diff_command(b, exclude);
                let scope = vet.then(|| VetScope {
                    scope: format!(
                        "核验对象:当前工作区相对 `{b}` 分支的代码改动({diff_cmd} 以及未提交改动)。只核实属于该改动范围的 findings;范围之外的既有问题不属于本次审查,一律驳回。"
                    ),
                    stdin: None,
                });
                (
                    schema_exec_args(
                        schema.clone(),
                        out_str.clone(),
                        format!(
                            "审查当前工作区相对 `{b}` 分支的代码改动(查看 {diff_cmd} 以及未提交改动),按 schema 输出 verdict(clean 或 changes_requested)和 findings{SUGGESTION_HINT}"
                        ),
                    ),
                    None,
                    scope,
                )
            }
            CodexAction::ReviewDoc => {
                let rel = doc_path.unwrap_or("");
                let content = std::fs::read_to_string(cwd.join(rel)).unwrap_or_default();
                let scope = vet.then(|| VetScope {
                    scope: format!(
                        "核验对象:随附设计文档 {rel}(文档内容已经由 stdin 附上,请基于文档内容核实,不要仅凭 findings 文本臆断)。"
                    ),
                    stdin: Some(content.clone()),
                });
                (
                    schema_exec_args(
                        schema.clone(),
                        out_str.clone(),
                        format!(
                            "审查随附设计文档 {rel} 并按 schema 输出 verdict/findings{SUGGESTION_HINT}"
                        ),
                    ),
                    Some(content),
                    scope,
                )
            }
            CodexAction::Ask => (
                vec![
                    "exec".into(),
                    "-s".into(),
                    "read-only".into(),
                    "-o".into(),
                    out_str.clone(),
                    ask_prompt.unwrap_or("").into(),
                ],
                None,
                None,
            ),
        };

        let out = self.run_codex(&args, stdin.as_deref(), "审查", control, on_progress, cwd)?;
        // codex 把最终结构化结果打到 stdout(0.144 起同时也写 -o)。
        // 故 stdout 优先:取最后一条能解析成 schema 的 JSON 行;读 -o 文件作 fallback。
        let parsed = parse_review_stdout(&out.stdout).or_else(|| parse_review_file(&out_file));
        // 退出码只在「拿不到任何可解析结果」时才有裁决权:
        // - 解析成功 → 采用,不因非零退出丢弃一份已到手的 verdict。
        // - 解析失败 + 退出 0 → 输出格式坏了,保持 fallback(ChangesRequested,fail-closed)。
        // - 解析失败 + 非零退出 → 进程本身崩了/没跑成,fail-loud 抛错走 step 失败决策门。
        //   此前这里把两者压成同一个 fallback:codex 崩溃被伪装成 changes_requested,
        //   until:codex-clean 永不收敛,fix 步骤拿着"(无法解析 Codex 输出)"这条空 finding
        //   在真实仓库自主写码,一路烧到 loop max(实测,见 spec)。
        let mut result = match parsed {
            Some(r) => r,
            None if !out.success => {
                return Err(EngineError::Cli(format!(
                    "Codex 审查进程异常退出,且没有产出可解析的结构化输出{}",
                    stderr_hint(&out.stderr_tail)
                )));
            }
            None => parse_failed_result(),
        };

        // 合法解析出 changes_requested 却零条目:模型自相矛盾的输出(schema 不禁止),
        // 与解析失败在收敛判定上同样保守,但根因不同 —— 显式提示避免用户把它误判成
        // "输出坏了"或反之(review finding #10)。
        if !result.parse_failed
            && matches!(result.verdict, Verdict::ChangesRequested)
            && result.items.is_empty()
        {
            on_progress(
                "⚠ codex 判定 changes_requested 但未给出任何 finding 条目(模型输出自相矛盾),收敛判定按保守处理",
                None,
            );
        }

        // P3(spec §3.3):自反驳核验。仅非 clean 且有结构化 items 时触发(fallback 路径
        // items 空,无从核起,保留原样);核验失败保留首轮 —— fail-closed 方向是"不丢
        // review 信号,宁可多修不可漏修"。
        if matches!(result.verdict, Verdict::ChangesRequested) && !result.items.is_empty() {
            if let Some(scope) = &vet_scope {
                on_progress("核验 findings(自反驳)…", None);
                match self.vet_pass(&result, scope, control, on_progress, cwd) {
                    Ok(mut vetted) => {
                        // vet 是第二次真实 codex 调用:metrics 与首轮求和而非覆盖 ——
                        // codex CLI 未来输出 usage 后,覆盖会让 vet step 两次计费只报
                        // 一次,budget 系统性低估(review finding #7,对齐 executor 的
                        // verify-retry sum 模式)。
                        vetted.metrics = StepMetrics::sum(result.metrics.take(), vetted.metrics);
                        result = vetted;
                    }
                    Err(e) => on_progress(&format!("核验失败,保留原 findings: {e}"), None),
                }
            }
        }

        // 用户裁决(2026-06-26):review/fix 每轮要看到详情。把结构化 verdict + 渲染好
        // 的 findings 作为 progress 行追发,UI 展开 step 输出就能直接看到完整审查结果
        // (而非埋在原始 stdout 的 JSON 末行里)。
        emit_findings_summary(&result, &mut |line| on_progress(line, None));

        Ok(result)
    }

    /// 二次 read-only codex 调用:逐条复核首轮 findings。输出不可解析 → Err
    /// (caller 保留首轮,不同于 review 主路径的 fallback-ChangesRequested 语义:
    /// vet 的 fallback 若替换首轮,等于用"无法解析"占位符抹掉真实 findings)。
    /// `scope` 重申主审查边界(review finding #5):核验是全新进程,不告诉它 base
    /// 分支 / 文档内容,它无从区分"本次改动的问题"与"范围外既有问题"。
    fn vet_pass(
        &self,
        first: &ReviewResult,
        scope: &VetScope,
        control: Option<&Control>,
        on_progress: &mut dyn FnMut(&str, Option<u32>),
        cwd: &Path,
    ) -> Result<ReviewResult, EngineError> {
        let (out_file, out_str) = out_file_for("codex-vet");
        let schema = write_schema()?;
        let prompt = format!(
            "{}\n以下是刚对上述核验对象给出的 code review findings。逐条重新核实:\
             尝试用具体代码/文档证据反驳每一条;删除证据不足、误读、幻觉或超出核验对象范围的条目,\
             保留确认成立的。驳回必须给出证据,不确定时保留。按 schema 重新输出最终 verdict 和 findings{SUGGESTION_HINT}\n\n{}",
            scope.scope, first.findings
        );
        let args = schema_exec_args(schema, out_str, prompt);
        let out = self.run_codex(&args, scope.stdin.as_deref(), "核验", control, on_progress, cwd)?;
        // vet 与 review 主路径的失败语义有意不同:核验的任何失败(非零退出 / 不可解析)
        // 都必须是可判别的 Err,让 caller 保留首轮结果;绝不能走"无法解析"占位符 fallback
        // 抹掉真实 findings。
        if !out.success {
            return Err(EngineError::Cli(format!(
                "Codex 核验进程非零退出{}",
                stderr_hint(&out.stderr_tail)
            )));
        }
        parse_review_stdout(&out.stdout)
            .or_else(|| parse_review_file(&out_file))
            .ok_or_else(|| EngineError::Cli("Codex 核验输出不可解析".into()))
    }

    /// 跑一次 codex exec 并做超时归类:run_command 到点 killpg 返回 success=false,
    /// 用墙钟区分超时与普通非零退出,超时 fail-closed 为 Err(executor 走 step 失败
    /// 决策门,不把超时喂回 loop 重挂)。非超时的非零退出原样返回,由调用方按各自
    /// 语义分类(review 主路径 → 解析 fallback;vet → fail-closed Err 保留首轮)。
    /// review 与 vet_pass 共用,防两处超时判定漂移。
    #[allow(clippy::too_many_arguments)]
    fn run_codex(
        &self,
        args: &[String],
        stdin: Option<&str>,
        label: &str,
        control: Option<&Control>,
        on_progress: &mut dyn FnMut(&str, Option<u32>),
        cwd: &Path,
    ) -> Result<CommandOutput, EngineError> {
        // codex exec 输出非 NDJSON 协议,原始行直接作无轮次进度上报(round=None)。
        let mut raw_sink = |line: &str| on_progress(line, None);
        let started = std::time::Instant::now();
        let out = run_command(
            &self.bin,
            args,
            cwd,
            stdin,
            Some(self.timeout_secs),
            control,
            &mut raw_sink,
        )?;
        if !out.success && started.elapsed() >= std::time::Duration::from_secs(self.timeout_secs) {
            // 可解释报错:大 diff 下 codex 逐文件读,实测每次工具调用约 6.7s,几百个文件
            // 就会撞上默认 20 分钟。给出两条出路,而不是只说"超时了"。
            return Err(EngineError::Cli(format!(
                "Codex {label}超时(>{}s),已中止。若目标 diff 很大,可调高 \
                 AGENTPIPE_CODEX_TIMEOUT_SECS(秒),或用 codex step 的 `exclude` \
                 排除 vendored / lock / 生成代码等无需审查的路径{}",
                self.timeout_secs,
                stderr_hint(&out.stderr_tail)
            )));
        }
        Ok(out)
    }
}

/// vet 二次核验的审查边界:主审查的 scope(base 分支 / 文档内容)重申给核验进程。
/// vet 是全新 codex exec,不重申它无从知道哪些 finding 属于本次审查范围
/// (review finding #5);review-doc 还需把文档内容重新经 stdin 附上。
struct VetScope {
    scope: String,
    stdin: Option<String>,
}

/// codex exec 结构化审查调用的公共参数前缀(read-only 沙箱 + strict schema + -o 输出)。
/// ReviewMr / ReviewDoc / vet_pass 三处共用 —— 此前三份手写 vec! 逐字重复,升级 CLI
/// 加公共 flag 时最易漏改不在原两处旁边的 vet(review finding #11)。Ask 无 schema 不适用。
fn schema_exec_args(schema: String, out_str: String, prompt: String) -> Vec<String> {
    vec![
        "exec".into(),
        "-s".into(),
        "read-only".into(),
        "--output-schema".into(),
        schema,
        "-o".into(),
        out_str,
        prompt,
    ]
}

/// 结构化输出临时文件路径:进程 + 递增序号命名,并发调用互不撞名。
/// 返回 (PathBuf, 字符串形式) 供 `-o` 参数与后续读取共用。
fn out_file_for(tag: &str) -> (PathBuf, String) {
    let seq = OUT_SEQ.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "agentpipe-{tag}-{}-{}.json",
        std::process::id(),
        seq
    ));
    let s = path.to_string_lossy().to_string();
    (path, s)
}

/// 把 ReviewResult 用人读形式追发到 on_progress:一行 verdict 摘要 + 每条 finding
/// 单独一行(含 ↳ 建议)。让 UI 展开 step 进度就能看到全部审查内容,不必去翻 audit
/// NDJSON 或 artifact 插值才能拿到 findings。
fn emit_findings_summary(result: &ReviewResult, on_line: &mut dyn FnMut(&str)) {
    let verdict_tag = match result.verdict {
        Verdict::Clean => "✓ 审查通过",
        Verdict::ChangesRequested => "⚑ 待修复",
    };
    let findings_trim = result.findings.trim();
    if findings_trim.is_empty() {
        on_line(&format!("─── {verdict_tag} ───"));
        return;
    }
    // 条数直接取结构化 items(单一来源,executor 的 residual 同源)。此前用行首 '['
    // 的文本启发式,summary 内嵌换行且续行以 '[' 开头(如 markdown checkbox)时会
    // 多算(review finding #8);fallback 路径 items 空 + 占位文案,两种算法同为 0。
    let count = result.items.len();
    let header = if count > 0 {
        format!("─── {verdict_tag} · {count} 条 finding ───")
    } else {
        format!("─── {verdict_tag} ───")
    };
    on_line(&header);
    for line in findings_trim.lines() {
        if !line.trim().is_empty() {
            on_line(line);
        }
    }
}

/// base ref 能否在 cwd 仓库解析为 commit。与 codex 实际跑的 `git diff {base}...HEAD`
/// 同一套 gitrevisions 规则(裸 ref,不做 `origin/` DWIM);`^{commit}` 确保解析到 commit-ish。
/// git 不可用 / 非 git 仓库 / ref 不存在一律返回 false → 调用方 fail-loud(绝不静默放过)。
///
/// 安全 + 容损(双层防御:① 字面 reject ② `--end-of-options` 强制后续 args 一律是 refs):
/// - `base` 以 `-` 开头:被当 git 选项(`--help` 退 0 印帮助;`--exec=` CVE 输入)。
/// - `base` 含空白 / 控制字符:多半是 LLM artifact 解析漂移(带换行 / 全角空格 /
///   markdown 代码块标记),让 rev-parse 喂到任何含空白的 ref 都必然失败,且错误
///   信息含原始字符让用户更难定位。这里 fail-loud 拒掉,引导上层 trim(executor 已 trim
///   首行,这条是兜底)。
///
/// `--end-of-options` 是 git 2.24+(2019),即便未来引入新选项也不混淆。
/// shell 元字符:exclude 条目会被 codex 抄进 `git diff ... -- '<pat>'` 里执行,
/// 单引号包裹只挡得住空白,挡不住引号闭合。任一出现即拒(白名单式思路的反面 —— 这里
/// 用黑名单是因为 pathspec 合法字符集很宽(`*` `?` `[` `]` `/` `.` `:` 都要留),
/// 而危险字符集小且封闭)。
const EXCLUDE_FORBIDDEN: &[char] = &[
    '\'', '"', '`', '$', ';', '&', '|', '<', '>', '(', ')', '{', '}', '\\', '!', '#', '\n', '\r',
];

/// 校验 exclude 条目。fail-loud:非法条目点名报错,不静默丢弃(丢弃 = 悄悄扩大审查
/// 范围或悄悄缩小,两种都让用户对"审了什么"失去判断)。
fn validate_exclude(exclude: &[String]) -> Result<(), EngineError> {
    for pat in exclude {
        let bad = |why: &str| {
            Err(EngineError::Cli(format!(
                "codex review 的 exclude 条目 {pat:?} 非法:{why}。\
                 exclude 会被拼进 codex 执行的 `git diff` pathspec,必须是纯路径模式\
                 (例:`**/vendor/**`、`docs/plans/`、`*.lock`)。"
            )))
        };
        if pat.is_empty() {
            return bad("空条目");
        }
        if pat.starts_with('-') {
            return bad("以 `-` 开头会被 git 当作 option");
        }
        if pat.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return bad("含空白或控制字符");
        }
        if let Some(c) = pat.chars().find(|c| EXCLUDE_FORBIDDEN.contains(c)) {
            return bad(&format!("含 shell 元字符 {c:?}"));
        }
    }
    Ok(())
}

/// 拼出让 codex 执行的 diff 命令。exclude 为空时逐字等于该特性引入前的命令。
fn diff_command(base: &str, exclude: &[String]) -> String {
    let mut cmd = format!("git diff {base}...HEAD");
    if !exclude.is_empty() {
        cmd.push_str(" --");
        for pat in exclude {
            cmd.push_str(&format!(" ':(exclude){pat}'"));
        }
    }
    cmd
}

fn base_ref_resolvable(cwd: &Path, base: &str) -> bool {
    if base.starts_with('-') {
        return false;
    }
    if base.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return false;
    }
    std::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["rev-parse", "--verify", "--quiet", "--end-of-options"])
        .arg(format!("{base}^{{commit}}"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// 占位词集合(reviewer 在没具体建议时常用的同义表达);整串(已 trim)小写匹配。
/// **收窄于 review-2 §D finding #9**:删 "no" / "-" / "todo" — code review 上下文
/// 这些常是合法短建议("No, use X instead" 被截 / 'TODO: 抽 helper' 简写 / markdown
/// 列表 '- xxx' 残留),整串等值匹配会误吞真实建议。保留高置信占位:n/a / none / 无 / tbd。
const SUGGESTION_PLACEHOLDERS: &[&str] = &["n/a", "none", "无", "tbd"];

/// suggestion 归一化:同时剥离 ASCII 空白 / 全角空白 / 包含中英标点的尾部装饰
/// (例 `"N/A."`、`"无。"`、`"None!"`、`"  无 "`)。
/// 修治 review §A finding #8:Rust `str::trim()` 只剥 Unicode whitespace,
/// 不动 `.` / `。` / `,` / `，` 等;LLM 给 schema-required 字段时为了"看起来像句子"
/// 常附标点,导致 'N/A.' 绕过 placeholder 检测、`"↳ 建议: N/A."`噪声喂下游 fixer。
fn normalize_suggestion(s: &str) -> String {
    s.trim_matches(|c: char| {
        c.is_whitespace() || ".,;:!?。，；：！？、…·•※()[]【】「」\"'`".contains(c)
    })
    .to_string()
}

/// 占位检测:输入应为 `normalize_suggestion` 归一后的小写串。
fn is_placeholder_suggestion(s: &str) -> bool {
    let lower = s.to_lowercase();
    SUGGESTION_PLACEHOLDERS.contains(&lower.as_str())
}

/// 单条 finding 渲染为人读行;suggestion 非空且非占位时附加 "↳ 建议: ..." 行,
/// 让下游 fixer prompt 直接看到可操作建议(spec §3.2)。
fn render_finding(f: &RawFinding) -> String {
    let head = format!("[{}] {}:{} {}", f.severity, f.file, f.line, f.summary);
    let s = normalize_suggestion(&f.suggestion);
    if s.is_empty() || is_placeholder_suggestion(&s) {
        head
    } else {
        format!("{head}\n  ↳ 建议: {s}")
    }
}

/// RawReview → ReviewResult(verdict 归一 + findings 扁平化 + items 结构化)。
/// 解析两路共用,避免漂移。metrics 始终 None:codex CLI 不在 stdout 输出 token
/// usage,等升级后填。items 的 suggestion 存原始串(渲染归一只在 render_finding)。
fn raw_to_result(raw: RawReview) -> ReviewResult {
    let verdict = if raw.verdict == "clean" {
        Verdict::Clean
    } else {
        Verdict::ChangesRequested
    };
    let items = raw
        .findings
        .iter()
        .map(|f| FindingItem {
            severity: Severity::parse_lossy(&f.severity),
            file: f.file.clone(),
            line: f.line,
            summary: f.summary.clone(),
            suggestion: f.suggestion.clone(),
        })
        .collect();
    let findings = raw
        .findings
        .iter()
        .map(render_finding)
        .collect::<Vec<_>>()
        .join("\n");
    ReviewResult { verdict, findings, items, parse_failed: false, metrics: None }
}

/// 从 codex stdout 抓最后一条能解析成 schema 的 JSON 行。无则 None(交给 -o fallback)。
fn parse_review_stdout(stdout: &str) -> Option<ReviewResult> {
    for line in stdout.lines().rev() {
        let t = line.trim();
        if t.starts_with('{') {
            if let Ok(raw) = serde_json::from_str::<RawReview>(t) {
                return Some(raw_to_result(raw));
            }
        }
    }
    None
}

/// schema 文件路径,进程内 memoize:REVIEW_SCHEMA 是编译期常量,旧实现每次调用都
/// 重写一个新临时文件(vet 开启后每轮 ×2)纯属浪费且泄漏临时文件。
static SCHEMA_PATH: OnceLock<String> = OnceLock::new();

fn write_schema() -> Result<String, EngineError> {
    if let Some(p) = SCHEMA_PATH.get() {
        // 命中缓存仍复核文件在盘:长驻进程(Tauri 多 run)期间 /tmp 可能被系统清理器
        // 清掉,盲信缓存会让后续所有 review/vet 拿死路径失败且无自愈,报错还被解析
        // fallback 掩盖成"无法解析"(review finding #4)。缺失则落回重写分支自愈。
        if Path::new(p).exists() {
            return Ok(p.clone());
        }
    }
    // 只缓存成功路径:失败不缓存,瞬时磁盘错误不会毒化长驻进程(Tauri GUI 同进程
    // 跑多个 run)的后续调用。写入走「唯一临时名 + 原子 rename」:同进程多线程并发
    // 首写同一目标时,读方(codex 子进程)永远看到完整内容,不会读到半写文件;
    // 目标名带 pid,跨进程仍互不干扰。
    let pid = std::process::id();
    let tmp = std::env::temp_dir().join(format!(
        "agentpipe-review-schema-{pid}-{}.tmp",
        OUT_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&tmp, REVIEW_SCHEMA)?;
    let path = std::env::temp_dir().join(format!("agentpipe-review-schema-{pid}.json"));
    std::fs::rename(&tmp, &path)?;
    Ok(SCHEMA_PATH
        .get_or_init(|| path.to_string_lossy().to_string())
        .clone())
}

/// 读 -o 文件解析;不可读 / 不可解析 → None(caller 决定 fallback 语义)。
fn parse_review_file(out_file: &Path) -> Option<ReviewResult> {
    let content = std::fs::read_to_string(out_file).ok()?;
    serde_json::from_str::<RawReview>(content.trim()).ok().map(raw_to_result)
}

/// 输出解析失败(但进程正常退出)时的 fail-closed 结果:当作 changes_requested,
/// 绝不静默判过。进程本身失败不走这里 —— 那是 Err,见 review() 的退出码裁决。
fn parse_failed_result() -> ReviewResult {
    ReviewResult {
        verdict: Verdict::ChangesRequested,
        findings: "(无法解析 Codex 输出,按需修改处理)".into(),
        items: vec![],
        parse_failed: true,
        metrics: None,
    }
}

// 必须是严格 JSON Schema:OpenAI 结构化输出要求每个 object 带 additionalProperties:false
// 且所有属性进 required,否则 API 报 invalid_json_schema(实测,见 docs/specs/cli-behavior-findings.md:18)。
//
// suggestion 字段(spec §3.2):reviewer 提供具体修改建议,提升下游 fixer 反馈深度。
// **必填**:OpenAI strict mode 要求所有 properties 都 required;RawFinding 端也去掉了
// `#[serde(default)]`,两层语义对齐(review §A finding #11 — 旧版 schema required +
// serde default 矛盾,生产 OpenAI 拒老 codex 在 schema 层、serde 反而 default 通过,
// 让 reader 困惑且 legacy guard 实际是死代码)。旧 codex 二进制需升级到支持新 schema,
// 与 agentpipe 锁版本策略一致(spec §3.2 follow-up:文档明示 min codex)。
const REVIEW_SCHEMA: &str = r#"{
  "type":"object","additionalProperties":false,
  "required":["verdict","findings"],
  "properties":{
    "verdict":{"type":"string","enum":["clean","changes_requested"]},
    "findings":{"type":"array","items":{
      "type":"object","additionalProperties":false,
      "required":["severity","file","line","summary","suggestion"],
      "properties":{
        "severity":{"type":"string","enum":["critical","major","minor","nit"]},"file":{"type":"string"},
        "line":{"type":"integer"},"summary":{"type":"string"},
        "suggestion":{"type":"string"}}}}
  }
}"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::Severity;

    fn raw(verdict: &str, sev: &str) -> RawReview {
        RawReview {
            verdict: verdict.into(),
            findings: vec![RawFinding {
                severity: sev.into(), file: "a.rs".into(), line: 1,
                summary: "s".into(), suggestion: "do x".into(),
            }],
        }
    }

    #[test]
    fn items_map_known_severities() {
        for (s, want) in [("nit", Severity::Nit), ("minor", Severity::Minor),
                          ("MAJOR", Severity::Major), ("critical", Severity::Critical)] {
            let r = raw_to_result(raw("changes_requested", s));
            assert_eq!(r.items[0].severity, want, "severity 串 {s}");
        }
    }

    #[test]
    fn unknown_severity_fail_closed_critical() {
        // stub / 旧二进制的 "high" 一类未知串必须按最严处理,不给 allow_residual 放行机会
        let r = raw_to_result(raw("changes_requested", "high"));
        assert_eq!(r.items[0].severity, Severity::Critical);
    }

    #[test]
    fn severity_ord_matches_threshold_semantics() {
        assert!(Severity::Nit < Severity::Minor);
        assert!(Severity::Minor < Severity::Major);
        assert!(Severity::Major < Severity::Critical);
    }
}

#[cfg(test)]
mod exclude_tests {
    use super::*;

    /// 回归:exclude 为空时 diff 命令必须与引入该特性之前逐字一致。
    #[test]
    fn empty_exclude_yields_todays_command_verbatim() {
        assert_eq!(diff_command("main", &[]), "git diff main...HEAD");
    }

    #[test]
    fn exclude_patterns_become_git_pathspec() {
        let ex = vec!["**/vendor/**".to_string(), "**/package-lock.json".to_string()];
        assert_eq!(
            diff_command("main", &ex),
            "git diff main...HEAD -- ':(exclude)**/vendor/**' ':(exclude)**/package-lock.json'"
        );
    }

    /// exclude 条目会被 codex 抄进 shell 命令执行 —— 等同把用户输入送进 shell。
    /// 每一类注入载荷都必须 fail-loud,且错误信息点名是哪一条。
    #[test]
    fn injection_payloads_are_rejected_fail_loud() {
        let bad = [
            "a'; rm -rf /; echo '",   // 引号闭合 + 命令注入
            "$(whoami)",               // 命令替换
            "`whoami`",                // 反引号
            "a; whoami",               // 分号
            "a | whoami",              // 管道
            "a && whoami",             // 逻辑与
            "a > /tmp/x",              // 重定向
            "a\nwhoami",               // 换行
            "a b",                     // 空白
            "--upload-pack=x",         // 以 - 开头,会被 git 当 option
            "",                        // 空串
        ];
        for pat in bad {
            let err = validate_exclude(&[pat.to_string()])
                .expect_err(&format!("必须拒绝: {pat:?}"));
            let msg = err.to_string();
            assert!(
                msg.contains("exclude"),
                "错误信息须点名 exclude 字段: {msg}"
            );
        }
    }

    #[test]
    fn ordinary_pathspecs_are_accepted() {
        let ok = ["**/vendor/**", "docs/plans/", "*.lock", "packages/x/generated"];
        for pat in ok {
            validate_exclude(&[pat.to_string()]).unwrap_or_else(|e| panic!("应接受 {pat:?}: {e}"));
        }
    }
}

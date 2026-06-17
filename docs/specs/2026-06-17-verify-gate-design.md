# 通用校验门(verify gate):让"完成"从退出码升级到目标达成(设计)

日期 2026-06-17。配套问题讨论见对话;本 spec 落地"方案 A:泛化校验门"。

## 1. 问题

auto 模式下,一步的"完成"判据是**子进程退出码**(`runner/mod.rs` 的 `status.success()`)。exit 0 只代表 CLI 正常退出,不代表它真把这步的目标干好了——claude 可能潦草收尾 / 跑偏,流水线照样 march 到下一步,错误顺流而下。这是当前设计真正脆弱处(控制流本身的 for 循环很稳,问题在语义层)。

现状里唯一的"语义校验"是 `Loop { until: "codex-clean" }`:把 `[codex-review, apply-feedback]` 反复跑到 codex 审查 clean。它本质是一个**手搓的 verify-retry**,但只服务 loop body、只认 codex-clean、配置笨重。

目标:把这套机制提炼成**任意 agent 步骤可挂的一等 verify gate**——步骤跑完后由一个 verifier 判"目标是否达成",未达成则带反馈重试,耗尽后按策略升级。

## 2. 现有可复用资产

- `CodexRunner.review` 已产出**严格 schema 的结构化判据** `Verdict { Clean, ChangesRequested }` + `findings`(`runner/codex.rs`),是现成的可机读 pass/fail。
- `StepOutput { artifact, findings, verdict }` + `{{step.field}}` 插值(`context.rs`)已能把 findings 喂给下游 prompt——codex-loop 的 apply-feedback 正是 `{{rev.findings}}`。
- executor 的 claude/codex 分支已有一个 `loop { run; Ok=>finish/return; Err=>handle_failure{Retry=>continue} }` 的**失败重试循环**,verify-retry 可直接寄生其中(见 §5)。

## 3. 候选形状(都属"方案 A"范畴,挑落地形态)

### A1. Step 内联 `verify` 字段(推荐)

给 claude/codex 这类"干活步骤"加一个可选 `verify` 块,声明用谁、判什么、重试几次、不过怎么办。

- 优点:声明式、贴着具体步骤、直接复用 Verdict/findings;executor 改动集中在一个 verify 子环;codex-loop 是它的特例(§6),概念收敛成一套。
- 缺点:Step schema 加字段;executor 干活分支要织入 verify 子环。

### A2. 把 `Loop.until` 泛化成富谓词,单步校验 = size-1 loop

保留一切都是 loop,扩 `until` 支持 `claude-verify` / 自定义谓词,单步校验就包一层 loop。

- 优点:不加新概念,复用 loop。
- 缺点:每个要校验的步骤都强行包 loop,body 与 verifier 的耦合很别扭;声明冗长;"一步带校验"被迫表达成"循环"。语义错位。

### A3. 新 StepKind `Verify`(独立校验步骤,可触发上一步重跑)

- 缺点:当前线性 executor 里"一个步骤回头重跑上一个步骤"很别扭,要引入跨步回跳的控制流,代价最大。否决。

### 结论:A1

A1 在"贴合语义 + 复用现成 verdict + executor 改动可控"上最优。codex-loop 保留不动(向后兼容),未来可在 A1 之上重表达(§6)。

## 4. Schema 设计(A1)

`StepKind::Claude` 与 `StepKind::Codex` 各加一个 `#[serde(default)] verify: Option<Verify>`:

```yaml
- id: implement
  kind: claude
  prompt: "按执行文档实现…"
  verify:
    by: codex                 # Phase 1 仅 codex(结构化 verdict 现成);claude 留 Phase 2
    action: review-mr         # 复用 CodexAction:review-mr | review-doc | ask
    base: dev                 # review-mr 用
    # path: docs/x.md         # review-doc 用
    # prompt: "判定是否达成目标X,clean=达成"   # ask 用
    max_retries: 2            # 未达成时重跑干活步骤的次数上限(0 = 纯质量门,不重试)
    on_unmet: gate            # 重试耗尽后:gate(默认,弹决策门等人) | fail | continue
    feedback: true            # 默认 true:重试时把 verifier findings 作为反馈注入干活 prompt
```

Rust 类型:

```rust
#[derive(Debug, Deserialize, Serialize)]
pub struct Verify {
    pub by: Verifier,                 // enum { Codex }(Phase 1)
    #[serde(flatten)]
    pub spec: VerifierSpec,           // 复用 codex 的 action/base/path/prompt
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,             // 默认 2,validate 限 <= 上限防 runaway
    #[serde(default)]
    pub on_unmet: OnUnmet,            // enum { Gate(默认), Fail, Continue }
    #[serde(default = "default_true")]
    pub feedback: bool,
}
```

> `VerifierSpec` 直接复用 codex 那几个字段(action/base/path/prompt),避免再造一套;validate 也复用现有 codex 字段校验。

## 5. Executor 语义(寄生现有重试环)

干活步骤(claude/codex)分支的 `loop` 改造,verify 织在 `Ok` 之后、`finish` 之前:

```
attempt = 0
feedback = None
loop {
    let prompt_i = interpolate(prompt) + feedback_block(feedback)   // feedback 注入
    match run(prompt_i) {
        Err(e) => handle_failure(...)                              // 失败路径不变
        Ok(out) => {
            record(artifact = out.answer)
            match verify_step(&verify) {                           // 无 verify → 视为 Met
                Met            => { finish(metrics); return Ok }
                Unmet{findings} if attempt < max_retries => {
                    emit StepProgress("校验未通过,第 {attempt+1} 次重试")
                    record(findings); feedback = Some(findings); attempt += 1; continue
                }
                Unmet{findings} => match on_unmet {                // 耗尽
                    Gate     => decision gate(等人:重试/跳过/中止)    // 复用 handle_failure 的 gate 通道
                    Fail     => { fail(step); return Err }
                    Continue => { finish(带"未达标"告警 summary); return Ok }
                }
            }
        }
    }
}
```

- `verify_step` 内部就是 `CodexRunner.review(spec)`,返回 `Verdict`。`Clean → Met`,`ChangesRequested → Unmet{findings}`。
- **失败重试**(Err,进程非零)与 **校验重试**(Unmet)共用同一个 `loop`,但用各自计数;两者都受 control.is_aborted 兜底。
- `feedback_block`:`feedback=true` 时把 findings 拼成"上一轮校验反馈:\n{findings}\n请据此修正后重做"追加到 prompt;`false` 则只重跑不带反馈。

## 6. 与 codex-loop 收敛(altitude)

深层洞察:**codex-loop 就是手搓的 verify-retry**,verify gate 才是底层原语。

- Phase 1:不动 codex-loop(`Loop{until:"codex-clean"}` 原样保留,向后兼容存量 task.yaml)。
- 未来(本 spec 不实施):`implement` 步骤挂 `verify:{by:codex, action:review-mr, on_unmet:gate}` 即可替代 `[implement] + codex-loop[review,apply-feedback]` 整段——loop 退化成 sugar。届时给出迁移说明,不强制。

这条写进 spec 是为了**明确两者不是并列的两套机制**,而是 sugar 与 primitive 的关系,防后续各长各的。

## 7. 事件 / UI

最小扩面,复用现有通道:

- 校验过程用 `StepProgress { line, round: None }` 叙述("校验中…" / "校验未通过,第 N 次重试" / "校验通过")。UI 已能渲染进度行(2026-06-17 进度可见性那轮)。
- `on_unmet: gate` 复用既有 `StepAwaitingGate { gate_kind: Decision }` 通道,UI 的 GatePrompt 无需改。
- 终态是否"经校验"可选地进 `StepFinished.summary`("done · 已校验" / "done · 未达标(continue)"),不新增协议字段。

> 暂不新增 `StepVerify` 专用事件:进度行 + 决策门已覆盖,先不扩协议;若后续要在 UI 上做"校验中"独立子态再议。

## 8. 校验与 fail-closed

- `verify.by` 非 codex(Phase 1)→ validate 报错(claude verifier 留 Phase 2)。
- `verify.spec` 复用 codex 字段校验(review-mr 需 base / review-doc 需 path / ask 需 prompt)。
- `max_retries` 限上限(如 <= 10),防 runaway。
- verify 只能挂 claude/codex(干活步骤),挂 human/loop → validate 报错。
- **verifier 自身失败 / 输出不可解析** → 复用 codex `parse_review` 的 fail-closed,落 `ChangesRequested`,即视为 Unmet(绝不静默判过)。
- `on_unmet` 默认 `Gate`(最保守:交人决策),对齐设计基线"判定失败走最保守分支"。
- Abort 期间不弹 verify gate(沿用 `handle_failure` 里 `is_aborted → Abort` 的先判)。

## 9. 向后兼容

- `verify` 是 `Option`,缺省 = 无校验,存量 task.yaml 行为完全不变。
- codex-loop 不动。
- `StepKind::Claude/Codex` 加 optional 字段属向后兼容演进。

## 10. 不做(划界)

- claude-as-verifier(claude 产结构化 verdict)→ Phase 2(需给 claude 加可机读判据:trailing `VERDICT:` 哨兵行 / 工具 / stream-json + schema,另议)。
- 跨步回跳、DAG、并行步骤 → 超范围。
- 自动把 codex-loop 迁成 verify gate → 仅文档说明路径,不强制迁。
- verifier 结果进上下文供任意下游引用(目前只注入到本步重试)→ 需要再加 `{{self.findings}}` 暴露,Phase 2。

## 11. 测试

- manifest:`verify` 解析 + 校验(by 非 codex 拒绝 / 缺字段拒绝 / 挂 human 拒绝 / max_retries 越界拒绝)。
- executor(stub):
  - verify Clean → 一次通过,无重试。
  - verify ChangesRequested → 重试,达 max 后按 on_unmet(gate/fail/continue 三分支)。
  - feedback 注入:重试的 prompt 含上一轮 findings(stub 回显验证)。
  - verifier 解析失败 → fail-closed Unmet。
  - Abort 期间不弹 verify gate。
- stub-codex 已支持 `STUB_VERDICT` 切 clean/changes_requested,直接复用。

## 12. 分 Phase

- Phase 1(已落地):schema(Verify 类型 + validate)→ executor verify 子环 → 进度叙述 → 测试。codex verifier only。
- Phase 2(已落地):
  - **claude verifier**:`by: claude` + `prompt`(判定指令)+ 可选 `skill`。executor 以 `--permission-mode plan`(只读,fail-closed:verifier 不改 target,实测见 cli-behavior-findings)跑 claude,回复末行 `VERDICT: pass|fail`,`parse_verdict` 末行优先解析,无哨兵 fail-closed 为未达成。
  - **findings 暴露**:verify 跑完把 verifier findings 记进该步 `StepOutput.findings`,下游可 `{{<step-id>.findings}}` 引用(注:插值无 `self` token,用步骤自身 id)。
  - schema 调整:`Verify.action` 由必填改 `Option`(codex 需要、claude 不需要);新增 `skill`;`Verifier` 加 `Claude`。因 Phase 1 未发布,直接演进无迁移负担。

## 13. codex-loop → verify 迁移(指引,不强制)

codex-loop 是手搓的 verify-retry,verify gate 是其底层原语。一段:

```yaml
# 旧:implement + 显式 codex-loop
- id: implement
  kind: claude
  prompt: 按执行文档实现
- id: codex-loop
  kind: loop
  until: codex-clean
  max: 5
  body:
    - id: codex-review-mr
      kind: codex
      action: review-mr
      base: dev
    - id: apply-feedback
      kind: claude
      prompt: "修 {{codex-review-mr.findings}}"
```

可等价收敛为:

```yaml
# 新:implement 自带 verify
- id: implement
  kind: claude
  prompt: 按执行文档实现
  verify:
    by: codex
    action: review-mr
    base: dev
    max_retries: 5
    on_unmet: gate   # 旧 loop 到 max 未净是静默放行,verify 默认更保守(交人)
```

语义差异需注意:
- 旧 loop 到 max 仍未 clean 时**静默继续**(`LoomMaxReached` 后 `Ok`);verify 默认 `on_unmet: gate` 更保守。要保留旧"放行"语义用 `on_unmet: continue`。
- 旧 loop 的 review 与 fix 是两个独立 step(各自事件/artifact);verify 把它们融进一个 step,findings 走 `{{implement.findings}}`。
- 迁移**不强制**:codex-loop 仍受支持(`Loop{until:codex-clean}` 不动),存量 task.yaml 零改动。新写流程推荐用 verify。

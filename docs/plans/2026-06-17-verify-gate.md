# 实施计划:通用校验门(verify gate)

配套 spec:[../specs/2026-06-17-verify-gate-design.md](../specs/2026-06-17-verify-gate-design.md)。

## Phase 1 范围收敛(相对 spec 的两点 trade-off)

1. **verify 仅挂 `claude` 步骤**(spec 写 claude/codex 都行)。理由:codex 步骤本身就是 verifier/reviewer,"校验一个 verifier"价值低,且 codex review-mr 无 prompt 可注入反馈,feedback 注入点别扭。schema 只给 `StepKind::Claude` 加 `verify`;codex-step verify 留 Phase 2。
2. **UI 暂不做 verify 编辑器**。引擎 + manifest 支持 verify,模板/手写 yaml 可用;Composer 里增删改 verify 的 UI 留 Phase 2(spec §7:校验过程本就只走进度行,无新协议)。

## 步骤

### 1. manifest.rs:schema + 校验

- 新增类型:
  - `Verifier { Codex }`(`#[serde(rename_all="lowercase")]`)。
  - `OnUnmet { Gate(default), Fail, Continue }`。
  - `Verify { by, action: CodexAction, base?, path?, prompt?, max_retries(默认2), on_unmet, feedback(默认true) }`。
  - 辅助:`default_max_retries()->2`、`default_true()->true`。
- `StepKind::Claude` 加 `#[serde(default)] verify: Option<Verify>`。
- `validate_step` 的 Claude 分支:if let Some(v)=verify → 校验 codex 字段(review-mr 需 base / review-doc 需 path / ask 需 prompt,复用现有 codex 校验逻辑抽个 `validate_codex_fields`)+ `max_retries <= 10`。
- 注:verify 挂 human/loop 因 schema 无该字段,serde 静默忽略(不报错)——与 spec §8"报错"有出入,Phase 1 接受(成本不划算),plan 记此偏差。

### 2. executor.rs:verify 子环 + gate 复用

- 抽 `decision_gate(&self, step_id, suggestion) -> StepDecision`:emit `StepAwaitingGate{Decision}` + `commands.recv()` 映射。`handle_failure` 改为 emit `StepFailed` 后调 `decision_gate`(消除重复)。
- 新增 `verify_once(&self, v: &Verify, on_line) -> (Verdict, String)`:interpolate v 的 path/base/prompt → `self.codex.review(...)` → `(verdict, findings)`;`Err` fail-closed 为 `(ChangesRequested, "校验执行失败…")`。
- 改 `StepKind::Claude` 分支(现 152-182):在 `Ok(out)` 后织入 verify:
  - `verify` 为 None → 原行为(finish 带 metrics)。
  - 有 verify:on_line("校验中…") → `verify_once`:
    - `Clean` → on_line("校验通过") + finish("done · 已校验", metrics)。
    - `ChangesRequested`:`attempt < max_retries` → attempt++,on_line("校验未通过,第 N 次重试"),feedback=findings(若 v.feedback),continue;否则按 `on_unmet`:
      - `Continue` → finish("done · 未达标(continue)", metrics)。
      - `Fail` → fail + Err。
      - `Gate` → `decision_gate`:Retry(重置 attempt=0,带 feedback,continue)/ Skip(emit_skipped)/ Abort(Err)。
  - prompt 重跑时注入反馈:`feedback` 有值则在 interpolate 后追加"上一轮校验反馈:\n{f}\n请据此修正后重做。"
  - 失败重试(Err 路径)与校验重试共用同一 `loop`,attempt 独立计数;`control.is_aborted` 兜底沿用。
- 验证:`verify_once` 用 `self.codex`(免新 runner);`on_line` 是 progress_sink(不借 self,可同时喂 claude.run 与 codex.review)。

### 3. UI 类型镜像(最小)

- `ui/src/types.ts` 的 `StepKind` claude 加 `verify?: {...}`,仅为类型完整 / 不丢字段(round-trip);UI 不渲染编辑器。

### 4. 测试

- `manifest_test.rs`:verify 解析成功;校验失败分支(review-mr 缺 base / max_retries 越界)。
- `executor_test.rs`(stub,复用 `STUB_VERDICT`):
  - verify clean → 一次过,无重试,finish summary 含"已校验"。
  - verify changes_requested + max_retries:重试到上限,on_unmet=fail → RunStatus::Failed;on_unmet=continue → Success;on_unmet=gate(预置 ApproveGate/SkipStep)→ 对应行为。
  - feedback 注入:stub-claude 回显 prompt,断言重试那次 full_output 含 findings 片段(需 stub-codex 产出可辨识 findings;`STUB_VERDICT=changes_requested` 时 findings 走 stub-codex 的输出)。
- `cargo test --workspace` + `cargo clippy` 全绿。

### 5. 模板示例(可选)

- 给 `templates/` 加一个挂 verify 的最小示例(或在 full-pipeline 的 implement 步骤加 verify 注释),证明端到端。Phase 1 末做真机 smoke(真 claude + 真 codex)验证一次 unmet→retry→clean 闭环(烧 quota,按需)。

## 回滚

- 每步独立 commit。`verify` 全程 Option,缺省零行为变更;codex-loop 不动。Phase 1 出问题回滚 manifest+executor 两处即恢复。

## 四维自查(收尾)

链路:manifest 解析 → executor verify 子环 → 进度行/gate → UI。同构面:manifest.rs ↔ ui/types.ts(verify 字段)。字面vs语义:`Clean=达成`、attempt 计数边界、`on_unmet` 三分支。默认值最坏 case:verifier 失败 fail-closed Unmet、on_unmet 默认 Gate、max_retries 上限防 runaway。

## Phase 2(已落地)

1. **manifest.rs**:`Verify.action` 改 `Option`、加 `skill`、`Verifier` 加 `Claude`;validate 按 `by` 分支(codex 需 action、claude 需 prompt)。
2. **claude.rs**:`run` 加 `read_only: bool` → `--permission-mode plan`(只读)vs `bypassPermissions`。
3. **executor.rs**:`verify_once` 按 `by` 分支;claude verifier 拼判定 prompt + `read_only=true` 跑 + `parse_verdict` 解析末行 `VERDICT:`;verify 后把 findings 记入 `StepOutput.findings`(下游 `{{id.findings}}` 可用)。
4. **stub-claude.sh**:加 `STUB_CLAUDE_RESULT` env 驱动 verifier 测试的 result。
5. **测试**:manifest(claude verifier 解析 / 缺 prompt 拒绝)、executor(claude verifier pass/fail 流)、`parse_verdict` 单测(末行哨兵 / fail-closed)。
6. **真机 smoke**:真 claude work(建文件)+ claude verifier(plan 只读 Bash 查验 → `VERDICT: pass`)→ `已校验 · Success`,verifier 全程只读。
7. **文档**:spec §12/§13(codex-loop→verify 迁移指引)、cli-behavior-findings(plan 只读实测)。

验证:`cargo test --workspace`(11 个 test-result-ok)+ clippy 干净 + UI typecheck/13 测试/build 全绿。

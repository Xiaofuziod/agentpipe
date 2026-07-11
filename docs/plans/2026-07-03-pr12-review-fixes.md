# PR #12 评审修复执行文档

> 执行者:在 `feat/acp-hardening` 分支上继续工作(即 PR #12,禁止另开分支)。设计依据:[docs/specs/2026-07-03-pr12-review-fixes-design.md](../specs/2026-07-03-pr12-review-fixes-design.md)。一个 task 一个 commit,信息中文;每个 task 收尾双关卡全绿才 commit:`cargo test --workspace` + `cargo clippy --workspace --all-targets -- -D warnings`;触及 ui/ 的 task 另加 `cd ui && npm run build && npm run test`。

## Task 1 (R1): abort 终裁移到 join 点

Files: `crates/engine/src/runner/acp.rs`、`crates/engine/tests/acp_runner.rs`

1. 失败测试(acp_runner.rs):
```rust
#[test]
fn abort_during_permission_grant_never_reports_ok() {
    use agentpipe_engine::runner::acp::PermissionDecision;
    // 回调内先 request_abort 再批准:agent 会拿到授权正常完成(granted+EndTurn),
    // 但 run 期间 control 已中止 —— join 点终裁必须折为 Err,绝不能 Ok(设计 R1)。
    let control = Arc::new(Control::default());
    let command = mock_command();
    let full_cmd = format!("env MOCK_ACP_SCENARIO=permission_probe {command}");
    let runner = AcpRunner::with_timeout(
        AcpConfig { agent: "mock-perm-abort".into(), command: full_cmd }, 30);
    let cwd = std::env::current_dir().unwrap();
    let control_in_cb = control.clone();
    let mut cb = |_desc: &str| {
        control_in_cb.request_abort();
        PermissionDecision::Approve
    };
    let err = runner
        .run("go", Some(&control), &mut |_l, _r| {}, &cwd,
            agentpipe_engine::runner::acp::PermissionMode::Ask(&mut cb))
        .expect_err("control 已中止的 run 不得返回 Ok");
    assert!(format!("{err:?}").contains("中止"), "{err:?}");
}
```
2. 跑红:`cargo test -p agentpipe-engine --test acp_runner abort_during_permission_grant -- --nocapture`(当前可能间歇 Ok——正是竞态本体;断言必须稳定红/绿,实现后重复跑 10 次确认)。
3. 实现:`AcpRunner::run` 主线程 `result_rx.recv()` 之后加终裁:
```rust
match result_rx.recv() {
    Ok(r) => {
        // join 终裁(评审 P1):join happens-after worker 终态,此处判 abort 无竞态窗。
        // 只做 Ok→Err 单向折叠;worker 自身的 Err(超时/通信失败)保留原错误。
        if control.map_or(false, |c| c.is_aborted()) && r.is_ok() {
            return Err(EngineError::Cli("acp: 被用户中止".into()));
        }
        r
    }
    Err(_) => Err(EngineError::Cli("acp: 工作线程异常退出未返回结果".into())),
}
```
worker 侧既有 permission_abort guard 与收尾复查全部保留不动。
4. 验收:新测试连续 10 次绿(`for i in $(seq 10); do cargo test -p agentpipe-engine --test acp_runner abort_during_permission_grant || break; done`);既有 abort/timeout 测试全绿;双关卡。
5. commit: `fix(engine): acp abort 终裁移到 join 点 — 被中止的 step 不再误报成功计费(评审 P1)`

## Task 2 (R2): registry 按需加载

Files: `crates/engine/src/agents.rs`、`crates/cli/src/main.rs`、`src-tauri/src/commands.rs`

1. agents.rs 加(含单测:内联 command 的 manifest → false;按名 acp → true;loop body 内按名 acp → true):
```rust
/// manifest 是否真的依赖 registry(递归含 command: None 的 acp step)。
/// 不依赖时宿主应完全跳过 registry 读取 —— 坏 agents.toml 不得波及无关 run(评审 P2)。
pub fn needs_registry(manifest: &crate::manifest::Manifest) -> bool { /* 递归 walk,同 resolve_agents */ }
```
2. cli `load_manifest`:`needs_registry(&m)` 为真才 `load_default()?` + `resolve_agents`,否则跳过。
3. tauri:`prepare_manifest` 改名 `prepare_for_launch` 并加同样条件;`load_template` 本 task 先维持现状(Task 3 重划)。
4. 失败测试先行:坏 TOML(写进临时 AGENTPIPE_HOME,用 tests/common 的 EnvGuard + ENV_LOCK)+ 纯 claude manifest → run 正常;+ 按名 acp manifest → Err 含"解析失败"。放 `crates/engine/tests/manifest_test.rs` 或新集成测试,engine 层测 needs_registry + cli 层逻辑靠函数抽取可测化(把"条件加载+resolve"抽成 engine `agents::load_and_resolve_if_needed(&mut Manifest) -> Result<(), EngineError>`,cli/tauri 共用,单测打这个函数)。
5. commit: `fix(engine/cli/tauri): registry 按需加载 — 坏 agents.toml 不再阻断无 acp 的 run(评审 P2)`

## Task 3 (R3): resolve 只挂运行边界 + authoring/run 校验分档

Files: `crates/engine/src/manifest.rs`、`src-tauri/src/commands.rs`

1. manifest.rs:
```rust
enum ValidationMode { Authoring, Run }
pub fn validate(&self) -> Result<(), EngineError> { self.validate_impl(ValidationMode::Run) }
/// 作者态校验:容忍 acp command: None("还没配 registry"是运行前置条件,不是模板非法);
/// Some("") 空串仍拒;其余规则与 Run 完全一致。供 load/save 等读写边界使用。
pub fn validate_authoring(&self) -> Result<(), EngineError> { self.validate_impl(ValidationMode::Authoring) }
```
`validate_impl` 内 acp 分支:`command: None` 仅在 Run 档报错(文案不变)。
2. tauri:`load_template` = parse → `validate_authoring()`(不 resolve、不读 registry,返回原始模板);`save_manifest` = `validate_authoring()`;launch 走 Task 2 的 `prepare_for_launch`。
3. 测试:validate_authoring 容忍 None / 拒空串 / 其余规则仍生效(budget×allow_unmetered 在 authoring 档也拦);Run 档行为回归测试不变。
4. commit: `fix(engine/tauri): resolve 只挂运行边界 — load/save 不再烤死 command,按名模板可存可开(评审 P3)`

## Task 4 (R9): GateKind::Permission

Files: `crates/engine/src/protocol.rs`、`crates/engine/src/executor.rs`、`crates/cli/src/main.rs`、`ui/src/types.ts`、`ui/src/console/GatePrompt.tsx`、`ui/src/state/runReducer.ts`(+其测试)

1. protocol.rs `GateKind` 加 `Permission`(serde 已 lowercase)。
2. executor.rs:`decision_gate` 泛化:
```rust
fn gate_with_kind(&self, step_id: &str, suggestion: String, kind: GateKind) -> StepDecision { /* 原 decision_gate 体,gate_kind: kind */ }
fn decision_gate(&self, step_id: &str, suggestion: String) -> StepDecision {
    self.gate_with_kind(step_id, suggestion, GateKind::Decision)
}
```
权限回调路径(Ask 闭包内)改调 `gate_with_kind(step_id, suggestion, GateKind::Permission)`,suggestion 文案改"acp step 权限请求:{desc}。批准 / 拒绝 / 中止"。
3. cli main.rs `gate_command`:`GateKind::Decision | GateKind::Permission` 同臂(EOF → Abort);`prompt_gate` 对 Permission 显示 `批准(y+回车) / 拒绝(s) / 中止(其他)`。单测:Permission×EOF→Abort、y→ApproveGate、s→SkipStep。
4. ui:types.ts `GateKind` 加 `"permission"`;GatePrompt:approve 按钮文案 `decision→"重试"`、`permission→"批准"`、其余 `"批准"`;skip 按钮 `permission→"拒绝"`、其余 `"跳过"`;中止按钮显示条件扩为 `decision || permission`。runReducer 类型随 union 自动覆盖,测试补一条 permission gate case。
5. 执行器测试:acp_verify.rs 的 ask 集成测试断言 gate_kind 从 Decision 改为 Permission。
6. commit: `feat(engine/cli/ui): 权限门专属 GateKind::Permission — 按钮语义不再借决策门文案(评审 P8 门语义)`

## Task 5 (R4): 序列化洁净一次治本

Files: `crates/engine/src/manifest.rs`

1. `PermissionPolicy` 加 `fn is_reject(&self) -> bool`;`on_permission` 加 `skip_serializing_if = "PermissionPolicy::is_reject"`。
2. StepKind 与 Verify 全部缺 skip 的 Option 字段统一补 `skip_serializing_if = "Option::is_none"`(Claude skill/verify、Codex path/base/prompt、Human expects、Acp verify、Verify 的 action/base/path/prompt/skill/command)。
3. round-trip 测试:最小 acp step(`agent/command/prompt`)serde_yml 序列化输出断言不含 `on_permission` / `verify` / `skill` 键;含 `on_permission: ask` 的再序列化保留该键。既有测试若快照了带 null 的 YAML,按新输出修正快照(逐个确认是洁净化预期,不是行为回归)。
4. commit: `fix(engine): 模板序列化洁净 — Option/缺省字段不再带出 null 与 reject 噪声(评审 P4)`

## Task 6 (R6): mock 构建收敛进 tests/common

Files: `crates/engine/tests/common/mod.rs`、`crates/engine/tests/acp_runner.rs`、`crates/engine/tests/acp_verify.rs`

1. common/mod.rs 加 `pub fn mock_acp_agent_bin() -> String`(Once 构建 example + exists 断言 + 返回裸路径,内容取 acp_runner.rs 现实现)。
2. 两个测试文件 `mod common;`,删各自的 BUILD_MOCK/mock_command;scenario 一律调用点拼 `env MOCK_ACP_SCENARIO=<s> <bin>`,删 acp_verify 的 `.replace` 写法。
3. commit: `refactor(tests): mock acp 构建收敛 tests/common 单点(评审 P6)`

## Task 7 (R5): unmet-retry-met 链路测试

Files: `crates/engine/tests/acp_verify.rs`

1. 新测试:tempdir 里造 marker 路径,verify 用 `by: command`、`command: "test -f <marker> || { touch <marker>; exit 1; }"`、`max_retries: 1`;断言 RunStatus::Success、StepFinished summary 含"已校验"、StepProgress 里出现"第 1 次重试"。
2. commit: `test(engine): 补 acp verify unmet-retry-met 链路(spec §6,评审 P5)`

## Task 8 (R7): registry 路径 SSOT

Files: `crates/engine/src/paths.rs`、`crates/engine/src/agents.rs`、`crates/engine/src/manifest.rs`

1. paths.rs 加 `pub fn registry_path() -> PathBuf { base_dir().join("agents.toml") }`;agents.rs `load_default` 与 manifest.rs 缺 command 报错文案改用它(文案里 `registry_path().display()`)。
2. commit: `refactor(engine): registry 路径收敛 paths::registry_path 单点(评审 P7)`

## Task 9 (R8): GUI 跟进新字段

Files: `ui/src/types.ts`、`ui/src/composer/StepDrawer.tsx`、`ui/src/composer/StepCard.tsx`(verify 编辑器复用既有 `verifyEdit.ts`,不新造)

1. types.ts acp 变体:`{ kind: "acp"; agent: string; command?: string; prompt: string; verify?: Verify; on_permission?: "reject" | "ask" }`。
2. StepDrawer acp 分支:command 输入可选(空串 onChange 置 undefined,提示"留空则按 agent 名查 ~/.agentpipe/agents.toml");加 on_permission 下拉(reject 缺省,文案注明"ask = 权限请求经决策门问你,headless 下自动中止");"校验门(verify)"折叠段启用条件扩到 acp。
3. StepCard 新建 acp 默认值删 `command: ""`。
4. 验收:`cd ui && npm run build && npm run test`;手工:composer 建按名 acp step → 保存(经 Task 3 可存)→ 重新载入不带 command。
5. commit: `feat(ui): acp step 表单跟进 — command 可选/on_permission/verify 门(评审 P8 表单)`

## Task 10 (R10): spec 修订

Files: `docs/specs/2026-07-03-acp-hardening-design.md`

1. D2 消解段措辞:budget 未设时无签字环节、但无预算兜底预期可破,重跑受 max_retries + MAX_VERIFY_RETRIES 双约束。
2. D3 补记 GateKind::Permission;D4 补记 registry 按需加载与 authoring/run 分档。
3. commit: `docs(specs): acp-hardening spec 随评审修复修订 — D2 措辞/D3 权限门/D4 校验分档(评审 P9)`

## 收尾

- 双关卡 + ui 检查全绿后 push;在 PR #12 逐条回复 inline 评论(fixed + commit hash);PR 描述追加"评审修复"一节列 task↔commit 对照。
- 全部完成后输出:各 commit hash、关卡输出摘要、与本执行文档的偏差清单(无偏差也要显式说"无")。

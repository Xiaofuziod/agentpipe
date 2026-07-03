# PR #12 评审修复设计（Phase A 收尾）

日期：2026-07-03
状态：待实施
来源：PR #12 交叉评审（6 角度 finder + 对抗核验，评审记录见 PR review #4625114470）
落点：全部修复进 `feat/acp-hardening` 分支（review 修复进当前 MR；GUI 跟进与 spec 措辞修正一并收口，避免二次 review 周期）

## 1. 问题清单（评审确认项）

正确性：
- P1 abort 与 EndTurn 竞态：runner/acp.rs 的 `connect_fut` Ok 分支只 guard `permission_abort`，外部 Control abort 与 agent 自然收尾撞进同一 poll 轮时，被中止的 step 误发 `StepFinished{Done}` + 计费；末步时 run 级 Success 谎报。
- P2 agents.toml 爆炸半径：cli `load_manifest` 与 tauri `prepare_manifest` 无条件 `load_default()?`，坏 TOML 阻断不含任何 acp step 的 run。
- P3 save/load/resolve 不对称（潜伏）：load_template 经 resolve 把本机 command 烤进模板对象，一次 load→save 丢失按名可移植性；save_manifest 又存不了按名（command 省略）模板。

spec 验收缺口：
- P4 序列化洁净：`on_permission` / `verify` 无 `skip_serializing_if`，存模板带出 `on_permission: reject` / `verify: null`。
- P5 spec §6 的 unmet-retry-met 链路 executor 测试缺失。

清理：
- P6 acp_verify.rs 复制了 acp_runner.rs 的 mock 构建逻辑（发散变体更脆），tests/common 本为收敛而建。
- P7 registry 路径与"怎么补一条"建议在 manifest.rs / agents.rs 两处独立字面量。

follow-up（本轮一并收）：
- P8 GUI 未跟进：TS 侧 acp 仍 command 必填、无 verify / on_permission；权限门在 GUI 按钮文案"重试/跳过"与真实语义"批准/拒绝"错位。
- P9 spec 措辞：acp-hardening spec D2 "已被 D1 签字覆盖"在 budget 未设置时表述过强。

## 2. 设计决策

### R1（修 P1）abort 终裁移到 join 点

候选：
- a) 给 worker 的 Ok 分支和收尾复查补一个由 control-poll 路径也写入的共享 abort flag。缩窄但不消灭窗口——用户在主线程两次 poll 之间中止、worker 恰在此间完成，flag 仍未置位。
- b) 主线程 join 点终裁。【推荐】

决策（b）：`AcpRunner::run` 主线程 `result_rx.recv()` 拿到结果后，若 `control.is_aborted()` 为真，则无论 worker 返回 Ok 还是 Err，一律折为 `Err("acp: 被用户中止")`。join happens-after worker 终态，此判定无竞态窗口。语义边界：用户在 agent 已完成但尚未 join 的微小间隙里中止，也算中止（用户要停,宁可弃一份产物,fail-closed）。worker 侧既有 permission_abort guard 原样保留（它让 worker 提前退出省时间，终裁在 join 点兜底正确性）。

可确定性测试：`PermissionMode::Ask` 回调内先 `control.request_abort()` 再返回 Approve——agent 拿到授权正常完成，但 join 终裁必须折为中止 Err。

### R2（修 P2）registry 按需加载

- agents.rs 增加 `pub fn needs_registry(manifest: &Manifest) -> bool`：递归（含 loop body）存在 `command: None` 的 acp step 才为真。
- cli `load_manifest` 与 tauri 运行入口：`needs_registry` 为真才 `load_default()?` + `resolve_agents`；否则完全不碰 registry 文件。
- fail-loud 语义不变：真依赖 registry 的 manifest 遇坏 TOML 仍拒绝（spec D4 原意）；不依赖的不再被波及。明确否决"坏文件降级空表"——静默漏 resolve 违反 D4。

### R3（修 P3）resolve 只挂运行边界 + 校验分作者态/运行态

- `Manifest` 校验拆两档，内部 `validate_impl(mode)`，`enum ValidationMode { Authoring, Run }`：
  - Run = 现行 `validate()` 全量规则（含 acp command 必须 Some 非空）。公开 API `validate()` 语义不变，`Executor::try_new` 不动。
  - Authoring = 新增 `validate_authoring()`：唯一差异是容忍 acp `command: None`（"还没配 registry"是运行前置条件，不是模板非法）；`Some("")` 空串仍拒（作者态明确写错）；其余规则（budget×allow_unmetered、loop、verify 等）全保留。
- 入口重划：
  - 运行边界（cli run / cli validate 子命令 / tauri launch）：needs_registry 条件加载 → resolve → `validate()`(Run)。`agentpipe validate` 保持 Run 档——它回答"在这台机器能不能跑"。
  - 读写边界（tauri load_template / save_manifest）：不 resolve、不读 registry，`validate_authoring()`。load_template 返回未回填的原始模板，按名信息不再被烤掉；save 可存按名草稿。
- 不做：command 拆"声明值/解析值"双字段（更彻底但动协议与全部消费方，收益当前不成比例，记入 spec 备注留待 registry 生态铺开后再评）。

### R4（修 P4）序列化洁净一次治本

同根因散点一次扫：`StepKind` 与 `Verify` 里所有缺 `skip_serializing_if` 的 `Option` 字段统一补 `Option::is_none`（Claude 的 skill/verify、Codex 的 path/base/prompt、Human 的 expects、Acp 的 verify）；`on_permission` 加 `skip_serializing_if = "PermissionPolicy::is_reject"`（补 `fn is_reject`）。新增 round-trip 测试：最小 acp step 序列化输出不含 on_permission / verify / command 之外的默认噪声键。反序列化路径零影响。

### R5（修 P5）unmet-retry-met 测试

acp_verify.rs 增加：command verifier 用 stateful 脚本（tempdir marker：`test -f <marker> || { touch <marker>; exit 1; }`，首跑退 1、二跑退 0），断言 run Success、StepFinished 带"已校验"、progress 里出现过"第 1 次重试"。

### R6（修 P6）mock 构建收敛进 tests/common

`tests/common/mod.rs` 增加 `pub fn mock_acp_agent_bin() -> String`：Once 构建 `--example mock_acp_agent` + `exists` 断言 + 返回裸二进制路径。acp_runner.rs 与 acp_verify.rs 均 `mod common;` 复用；scenario 一律由调用点拼 `env MOCK_ACP_SCENARIO=<s> <bin>`，删除 acp_verify 的焊死 happy + `.replace` 掰回写法。

### R7（修 P7）registry 路径 SSOT

`paths.rs` 增加 `pub fn registry_path() -> PathBuf`（= `base_dir().join("agents.toml")`）；agents.rs `load_default` 与 manifest.rs 的缺 command 报错文案共用（文案改为 `registry_path().display()`，不再手拼 `/agents.toml`）。

### R8（修 P8-表单）GUI 跟进新字段

GUI 已有 claude 的 verify 编辑器（`ui/src/composer/verifyEdit.ts` + StepDrawer "校验门"折叠段），本轮是接线不是新造：
- `ui/src/types.ts`：acp 变体改 `{ kind: "acp"; agent: string; command?: string; prompt: string; verify?: Verify; on_permission?: "reject" | "ask" }`。
- StepDrawer acp 分支：command 输入改可选，placeholder/提示注明"留空则按 agent 名查 ~/.agentpipe/agents.toml"，onChange 空串时置 undefined（不发空串，对齐 Rust 侧 `Some("") 拒绝`）；新增 on_permission 下拉（默认 reject，说明文案带 fail-closed 语义）；verify 折叠段的启用条件从 claude-only 扩到 claude|acp。
- StepCard 新建 acp 默认值去掉 `command: ""`。

### R9（修 P8-门语义）GateKind::Permission

候选：
- a) 维持复用 Decision 门，GUI 靠 suggestion 文本纠偏。按钮文案错位不可修，误点即误授权。
- b) 协议加 `GateKind::Permission`，流程机制（Approve/Skip/Abort 命令、阻塞 recv）完全复用。【推荐】

决策（b）：
- protocol.rs `GateKind` 加 `Permission`（serde lowercase → "permission"）。审计兼容：旧日志无此值不受影响；本仓库单版本工具，无旧读者反向兼容问题。
- executor：`decision_gate` 泛化出 `fn gate_with_kind(&self, step_id, suggestion, kind) -> StepDecision`（入口 abort 短路、Abort 翻 Control 等全保留），`decision_gate` 委托 Decision；权限回调路径改用 Permission。
- cli `gate_command`：Permission 与 Decision 同组（EOF → Abort，fail-closed）；`prompt_gate` 对 Permission 显示"批准(y) / 拒绝(s) / 中止(其他)"文案。match 非穷尽会编译报错，天然防漏。
- ui：types.ts GateKind 加 "permission"；GatePrompt 对 permission 渲染 批准/拒绝/中止 按钮（发送的命令仍是 ApproveGate/SkipStep/Abort，仅文案换）；runReducer 与测试同步。
- spec D3 相应修订（见 R10）。

### R10（修 P9）spec 修订

`docs/specs/2026-07-03-acp-hardening-design.md`：
- D2 消解段"已被 D1 的 allow_unmetered 显式签字覆盖"改为"budget_usd 设置时被 D1 签字覆盖；未设置 budget 时无签字环节，但彼时本就无预算兜底预期，重跑仍受 max_retries 与 MAX_VERIFY_RETRIES 双重约束"。
- D3 补记：权限门使用专属 `GateKind::Permission`（本设计 R9），命令语义与决策门一致，EOF → Abort 不变。
- D4 补记：registry 按需加载（R2）与 authoring/run 校验分档（R3）语义。

## 3. 错误路径盘点

- R1：worker Err + control aborted → 仍 Err（终裁只会把 Ok 折 Err，不会反向）；control=None（测试路径）→ 终裁跳过，行为不变。
- R2：needs_registry 为真但文件不存在 → load_default 返回空表 → resolve no-op → Run validate 报缺 command（含 registry 出路提示），与现状一致。
- R3：save_manifest 收到 command:Some("") → authoring 校验拒绝（可解释报错）；load_template 打开含未知 agent 名的按名模板 → 成功打开（作者态合法），launch 时才报可解释错误。
- R9：旧审计日志回放（无 permission 值）不受影响；GUI 收到未知 gate_kind（版本错配）→ TS union 外值走 GatePrompt 现有 default 渲染（保底按 decision 处理，命令语义相同，不崩）。
- R4：只影响序列化输出；全部既有 YAML 解析路径不变。

## 4. 测试

- R1：Ask 回调内 abort-then-Approve 的确定性测试（join 终裁折 Err）；既有 abort/timeout 套件全保留。
- R2：坏 TOML + 纯 claude manifest → 正常运行不受阻；坏 TOML + 按名 acp manifest → fail-loud。
- R3：authoring 容忍 None / 拒绝空串；save→load round-trip 不烤 command（tauri 层单测或 engine 层 validate_authoring 单测 + commands 接线走查）。
- R4：最小 step 序列化快照（无噪声键）。
- R5：unmet-retry-met（见 R5 节）。
- R9：cli gate_command Permission×EOF→Abort 单测；ui GatePrompt/runReducer 测试更新。
- 全量关卡：`cargo test --workspace` + `cargo clippy --workspace --all-targets -- -D warnings` + `cd ui && npm run build`（或项目现行 ui 检查命令）。

## 5. 自审记录（写毕后走查）

- 链路连贯性：R2/R3 都动 manifest 加载链，合并设计为"运行边界=条件 registry+resolve+Run 校验；读写边界=无 registry+Authoring 校验"单一心智模型，cli 与 tauri 两宿主同构落地。
- 同构面：R4 一次扫全 StepKind 的 Option 字段而非只修被点名的两个；R9 的 gate 文案 cli/ui 两端同步改。
- 字面 vs 语义：R3 明确"validate() 语义不变"，防止 SDK 嵌入方被静默改语义；R1 明确终裁只 Ok→Err 单向。
- 默认值最坏 case：on_permission 缺省 reject 不变；GateKind 未知值 GUI 保底 decision 渲染；needs_registry=false 时坏文件完全不读，无新增静默分支。

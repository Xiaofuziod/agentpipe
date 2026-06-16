# CLI 行为实测结论(Task 1 spike)

日期 2026-06-16。环境:codex-cli 0.139.0,claude 2.1.178,macOS。runner 实现以此为准。

## 1. Codex 结构化审查输出

### 关键结论:用通用 `codex exec`,不用 `codex exec review` 子命令

- `codex exec review` 子命令**不接受 `-s/--sandbox`**(review 本就只读),误带会报 `unexpected argument '-s'`。
- 更要命:`codex exec review` 即便带 `--output-schema`,其 `-o`(--output-last-message)文件写的是**人类可读的 review 散文**("Review comment: - [P1] ...")**,不是 schema JSON**。靠它解析 verdict 必然失败 → fail-closed → 循环永不收敛。
- 通用 `codex exec -s read-only --output-schema <strict.json> -o <out>` "<review 指令>" 才把 `-o` 写成**严格符合 schema 的纯 JSON**。实测产出:
  ```json
  {"verdict":"changes_requested","findings":[{"severity":"high","file":"calc.py","line":5,"summary":"..."}]}
  ```

### schema 必须严格

OpenAI 结构化输出要求每个 object 带 `additionalProperties:false` 且所有属性进 `required`,否则 API 报:
```
{"param":"text.format.schema","code":"invalid_json_schema"}
```
runner 内嵌的 `REVIEW_SCHEMA` 已改严格版。

### runner 落地

`CodexRunner.review` 三个 action 全部走通用 `codex exec`:
- review-mr:`codex exec -s read-only --output-schema <s> -o <out> "审查相对 <base> 分支的改动…"`,让 codex 自己跑 `git diff`。
- review-doc:同上,文档内容经 stdin 喂入(`codex exec … -` 风格,stdin 作为 `<stdin>` block 追加)。
- ask:`codex exec -s read-only -o <out> "<prompt>"`。

## 2. Claude headless 自主写码

### allow_writes 用 `--permission-mode bypassPermissions`

- 实测 `claude -p --permission-mode bypassPermissions "创建文件… 然后运行 ls"`:**自主创建文件 + 跑 bash 都成功**。
- `acceptEdits` 只放行编辑、挡 bash;而 implement / apply-feedback 步骤需要 `git commit`(bash),故必须 `bypassPermissions`。
- runner 的 `allow_writes` 分支已从 acceptEdits 改为 bypassPermissions。

### 安全提醒

bypassPermissions 跳过所有审批,claude 在 target 仓库里完全自主。AgentPipe 已有两道约束:cwd 严格取自 manifest.target(不回退 home)、step 模式下 allow_writes 步骤(含 loop body)逐步门控。真机用 auto 模式跑 allow_writes 步骤等于完全放权,需用户自行确认 target 可信。

## 3. 待验(未烧 quota 的项)

- `claude -p "/skill-name …"` 能否可靠触发 skill:runner 当前用 `/{skill} {prompt}` 前缀,合理但未实测。design-review-claude(skill: four-dimension-review)首次真机跑时确认;若不触发,改为在 prompt 里点名调用。

## 4. 真机端到端

通用 exec + 严格 schema 的 review-mr 已用真 codex 验证 JSON 产出正确(见上)。runner 校准后,以本机真实 codex/claude 跑一遍 `tests/fixtures/sample-task.yaml` 的真机 smoke 再发布。

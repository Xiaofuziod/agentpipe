use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Clean,
    ChangesRequested,
}

#[derive(Debug, Default, Clone)]
pub struct StepOutput {
    pub artifact: Option<String>,
    pub findings: Option<String>,
    pub verdict: Option<Verdict>,
}

impl StepOutput {
    fn field(&self, name: &str) -> Option<String> {
        match name {
            "artifact" => self.artifact.clone(),
            "findings" => self.findings.clone(),
            "verdict" => self.verdict.as_ref().map(|v| match v {
                Verdict::Clean => "clean".into(),
                Verdict::ChangesRequested => "changes_requested".into(),
            }),
            _ => None,
        }
    }
}

pub struct RunContext {
    pub cwd: PathBuf,
    outputs: HashMap<String, StepOutput>,
    /// per-run USD 累计成本(所有 agent step 完成后累加 metrics.cost_usd)。
    cost_so_far_usd: f64,
    /// manifest 配置的 USD 上限;None = 不限。累计严格大于上限即 over budget。
    budget_usd: Option<f64>,
}

impl RunContext {
    pub fn new(cwd: PathBuf) -> Self {
        Self {
            cwd,
            outputs: HashMap::new(),
            cost_so_far_usd: 0.0,
            budget_usd: None,
        }
    }

    /// 设置 per-run USD 上限。**fail-loud**:非正/非有限值 panic(与 Executor::new 的
    /// manifest.validate().expect 同形,保持 safety-fail-closed 基线)。
    /// review-2 §C finding #3 修正:之前 silently 落 None 是 fail-OPEN 反「budget 是钱
    /// 类安全字段、默认 fail-closed」基线 — LangChain $47K 反模式正中靶心。生产路径
    /// 走 manifest.validate() → Executor::new → set_budget,合法值唯一来源,该 panic
    /// 实际不会被合法 caller 触发,只在测试 / SDK 误用时立刻报错。
    pub fn set_budget(&mut self, budget_usd: Option<f64>) {
        if let Some(b) = budget_usd {
            assert!(
                b.is_finite() && b > 0.0,
                "set_budget: budget_usd={b} 必须为正有限数(应先 Manifest::validate)"
            );
        }
        self.budget_usd = budget_usd;
    }

    pub fn cost_so_far_usd(&self) -> f64 {
        self.cost_so_far_usd
    }

    pub fn budget_usd(&self) -> Option<f64> {
        self.budget_usd
    }

    /// 累加一个 step 的成本。NaN / 负数 delta 被过滤(实测 claude 偶尔会输出 0,不应该出现负数),
    /// 防脏数据污染累计。返回累加**之后**的累计值,调用方可立刻 `is_over_budget()`。
    ///
    /// **可观测性**:遇到 NaN / 负数走 stderr warn(不静默吞 — review-fix §B),便于诊断
    /// 上游 runner 退化(如新 ACP agent 解析 cost 出 bug 一直送 NaN 时 budget 守护
    /// 形同虚设的反模式)。0.0 不算异常,无需 warn。
    pub fn add_cost(&mut self, delta_usd: f64) {
        if delta_usd.is_finite() && delta_usd >= 0.0 {
            self.cost_so_far_usd += delta_usd;
        } else {
            eprintln!(
                "[agentpipe] WARN: add_cost 丢弃无效 delta {delta_usd}(NaN/inf/负数),\
                 上游 runner 可能输出了脏 cost_usd,请排查"
            );
        }
    }

    /// 当前累计是否超 budget。无 budget 永远 false;有 budget 用 `>` 而非 `>=`
    /// 避免浮点抖动误杀刚好等于的情况。run() 主循环 / finish 都可读,单一来源。
    pub fn is_over_budget(&self) -> bool {
        matches!(self.budget_usd, Some(cap) if self.cost_so_far_usd > cap)
    }

    pub fn record(&mut self, step_id: &str, out: StepOutput) {
        self.outputs.insert(step_id.to_string(), out);
    }

    pub fn get(&self, step_id: &str) -> Option<&StepOutput> {
        self.outputs.get(step_id)
    }

    /// 替换所有 {{step-id.field}};未知引用替换为空串。
    pub fn interpolate(&self, template: &str) -> String {
        let mut result = String::with_capacity(template.len());
        let mut rest = template;
        while let Some(start) = rest.find("{{") {
            result.push_str(&rest[..start]);
            let after = &rest[start + 2..];
            if let Some(end) = after.find("}}") {
                let token = after[..end].trim();
                let value = token
                    .split_once('.')
                    .and_then(|(id, field)| self.get(id).and_then(|o| o.field(field)))
                    .unwrap_or_default();
                result.push_str(&value);
                rest = &after[end + 2..];
            } else {
                result.push_str("{{");
                rest = after;
            }
        }
        result.push_str(rest);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_unbounded_when_unset() {
        let mut ctx = RunContext::new(PathBuf::from("/tmp"));
        // 不设 budget,加任意多 cost 都不应 over
        ctx.add_cost(1000.0);
        ctx.add_cost(1.0);
        assert!(!ctx.is_over_budget());
        assert_eq!(ctx.cost_so_far_usd(), 1001.0);
    }

    #[test]
    fn budget_triggers_strict_gt() {
        let mut ctx = RunContext::new(PathBuf::from("/tmp"));
        ctx.set_budget(Some(1.0));
        // 刚好等于 cap 不触发(防浮点抖动)
        ctx.add_cost(1.0);
        assert!(!ctx.is_over_budget());
        // 任何额外 cost 都触发
        ctx.add_cost(0.001);
        assert!(ctx.is_over_budget());
    }

    #[test]
    fn budget_ignores_negative_and_nan_delta() {
        let mut ctx = RunContext::new(PathBuf::from("/tmp"));
        ctx.set_budget(Some(1.0));
        ctx.add_cost(-5.0);
        ctx.add_cost(f64::NAN);
        assert_eq!(ctx.cost_so_far_usd(), 0.0, "脏 delta 不污染累计");
        assert!(!ctx.is_over_budget());
        ctx.add_cost(0.5);
        assert_eq!(ctx.cost_so_far_usd(), 0.5);
        assert!(!ctx.is_over_budget());
    }
}

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

    /// 设置 per-run USD 上限(Executor::new 调用)。负数 / NaN / 0 由 manifest validate 拦截,
    /// 此处只接受 caller 已校验过的合法值。
    pub fn set_budget(&mut self, budget_usd: Option<f64>) {
        self.budget_usd = budget_usd;
    }

    pub fn cost_so_far_usd(&self) -> f64 {
        self.cost_so_far_usd
    }

    pub fn budget_usd(&self) -> Option<f64> {
        self.budget_usd
    }

    /// 累加一个 step 的成本,返回累加**之后**是否超额(`cost_so_far > budget`)。
    /// 用 `>` 而非 `>=` 避免浮点抖动误杀刚好等于的情况。无 budget 永远返 false。
    pub fn add_cost_and_check(&mut self, delta_usd: f64) -> bool {
        // 防 NaN / 负数 cost 污染累计(实测 claude 偶尔会输出 0,不应该出现负数)
        if delta_usd.is_finite() && delta_usd > 0.0 {
            self.cost_so_far_usd += delta_usd;
        }
        match self.budget_usd {
            Some(cap) => self.cost_so_far_usd > cap,
            None => false,
        }
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
        assert!(!ctx.add_cost_and_check(1000.0));
        assert!(!ctx.add_cost_and_check(1.0));
        assert_eq!(ctx.cost_so_far_usd(), 1001.0);
    }

    #[test]
    fn budget_triggers_strict_gt() {
        let mut ctx = RunContext::new(PathBuf::from("/tmp"));
        ctx.set_budget(Some(1.0));
        // 刚好等于 cap 不触发(防浮点抖动)
        assert!(!ctx.add_cost_and_check(1.0));
        // 任何额外 cost 都触发
        assert!(ctx.add_cost_and_check(0.001));
    }

    #[test]
    fn budget_ignores_negative_and_nan_delta() {
        let mut ctx = RunContext::new(PathBuf::from("/tmp"));
        ctx.set_budget(Some(1.0));
        assert!(!ctx.add_cost_and_check(-5.0));
        assert!(!ctx.add_cost_and_check(f64::NAN));
        assert_eq!(ctx.cost_so_far_usd(), 0.0, "脏 delta 不污染累计");
        assert!(!ctx.add_cost_and_check(0.5));
        assert_eq!(ctx.cost_so_far_usd(), 0.5);
    }
}

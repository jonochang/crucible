use crate::context::GatheredContext;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FocusAreas {
    pub summary: String,
    pub focus_items: Vec<FocusItem>,
    pub trade_offs: Vec<String>,
    #[serde(default)]
    pub affected_modules: Vec<String>,
    #[serde(default)]
    pub call_chain: Vec<String>,
    #[serde(default)]
    pub design_patterns: Vec<String>,
    #[serde(default)]
    pub reviewer_checklist: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FocusItem {
    pub area: String,
    pub rationale: String,
}

#[derive(Debug, Clone)]
pub struct AgentContext {
    pub diff: String,
    pub gathered: GatheredContext,
    pub focus: Option<FocusAreas>,
    pub dep_graph: Option<String>,
}

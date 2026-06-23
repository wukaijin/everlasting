#![cfg(test)]

use crate::agent::permissions::{risk_for_tool, Risk};

#[test]
fn risk_for_tool_categorization() {
    assert_eq!(risk_for_tool("read_file"), Risk::Low);
    assert_eq!(risk_for_tool("grep"), Risk::Low);
    assert_eq!(risk_for_tool("write_file"), Risk::Medium);
    assert_eq!(risk_for_tool("edit_file"), Risk::Medium);
    assert_eq!(risk_for_tool("shell"), Risk::High);
    assert_eq!(risk_for_tool("web_fetch"), Risk::Low);
}

#[test]
fn risk_label_cn_is_full_text() {
    assert_eq!(Risk::Low.label_cn(), "低");
    assert_eq!(Risk::Medium.label_cn(), "中");
    assert_eq!(Risk::High.label_cn(), "高");
    assert_eq!(Risk::Critical.label_cn(), "极高");
}

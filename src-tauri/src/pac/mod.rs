//! PAC 自动代理模块。
//!
//! 为「反向 PAC」方案提供基础能力：根据名单生成 PAC 脚本
//! （[`generator`]）、解析/拉取 gfwlist 域名（[`gfwlist`]）、
//! 以及一个本地最小 HTTP 服务托管 `proxy.pac`（[`server`]）。

pub mod generator;
pub mod gfwlist;
pub mod server;

use serde::{Deserialize, Serialize};

/// PAC 名单。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PacList {
    /// 域名（后缀匹配）：google.com / .google.com 均按后缀处理。
    pub domains: Vec<String>,
    /// IP/CIDR：1.2.3.0/24、1.2.3.4。
    pub ip_cidrs: Vec<String>,
    /// 顶级后缀：.hk / .tw。
    pub suffixes: Vec<String>,
    /// 地区预置（如 hk / tw）——解析为预置 IP 段参与匹配。
    pub regions: Vec<String>,
}

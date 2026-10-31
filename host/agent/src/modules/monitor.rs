//! Phase 3: `stats.*`, `alerts.*`, `process.*`, `profiles.*`.

use async_trait::async_trait;
use serde_json::{Value, json};

use super::{internal, typed_param, u64_param};
use crate::p3::{self, AlertRule, Profile};
use crate::platform::{Priority, blocking};
use crate::router::{MethodSpec, Module, RequestContext, RpcError, Tier};

pub struct MonitorModule;

const fn m(name: &'static str, min_tier: Tier) -> MethodSpec {
    MethodSpec { name, min_tier }
}

const METHODS: &[MethodSpec] = &[
    m("stats.subscribe", Tier::Viewer),
    m("stats.unsubscribe", Tier::Viewer),
    m("alerts.get", Tier::Viewer),
    m("alerts.set", Tier::Admin),
    m("process.list", Tier::Viewer),
    m("process.kill", Tier::Admin),
    m("process.priority", Tier::Admin),
    m("profiles.get", Tier::Viewer),
    m("profiles.set", Tier::Remote),
];

fn pid_param(params: &Value) -> Result<u32, RpcError> {
    let pid = u32::try_from(u64_param(params, "pid")?)
        .map_err(|_| RpcError::bad_params("pid out of range"))?;
    helm_win_sysmon::process::check_target(pid, std::process::id())
        .map_err(crate::platform::sysmon_err)?;
    Ok(pid)
}

#[async_trait]
impl Module for MonitorModule {
    fn methods(&self) -> &'static [MethodSpec] {
        METHODS
    }

    async fn call(
        &self,
        method: &str,
        params: Value,
        ctx: &RequestContext,
    ) -> Result<Value, RpcError> {
        let state = ctx.state.clone();
        let p = state.platform.clone();
        match method {
            "stats.subscribe" => {
                let interval = p3::stats_interval(&params)?;
                state
                    .p3
                    .subscribe_stats(&state, ctx.session.clone(), interval);
                Ok(json!({}))
            }
            "stats.unsubscribe" => {
                state.p3.unsubscribe_stats(ctx.session_id);
                Ok(json!({}))
            }
            "alerts.get" => {
                let rules = blocking(move || state.p3.alert_rules().map_err(internal)).await?;
                Ok(json!({"rules": rules}))
            }
            "alerts.set" => {
                let rules: Vec<AlertRule> = typed_param(&params, "rules")?;
                p3::validate_rules(&rules).map_err(RpcError::bad_params)?;
                let n = rules.len();
                let st = state.clone();
                blocking(move || st.p3.set_alert_rules(rules).map_err(internal)).await?;
                state.audit.record(
                    "alerts_set",
                    Some(&ctx.device_id),
                    Some(&ctx.peer),
                    &format!("rules={n}"),
                );
                Ok(json!({}))
            }
            "process.list" => {
                let list = blocking(move || p.processes()).await?;
                let processes: Vec<Value> = list
                    .iter()
                    .map(|p| {
                        json!({"pid": p.pid, "name": p.name, "exe": p.exe, "cpu_pct": p.cpu_pct,
                               "ram_bytes": p.ram_bytes, "parent_pid": p.parent_pid,
                               "is_elevated": p.is_elevated, "priority": p.priority})
                    })
                    .collect();
                Ok(json!({"processes": processes}))
            }
            "process.kill" => {
                let pid = pid_param(&params)?;
                blocking(move || p.kill_process(pid)).await?;
                state.audit.record(
                    "process_kill",
                    Some(&ctx.device_id),
                    Some(&ctx.peer),
                    &format!("pid={pid}"),
                );
                Ok(json!({}))
            }
            "process.priority" => {
                let pid = pid_param(&params)?;
                let level: Priority = typed_param(&params, "level")?;
                if level == Priority::Realtime {
                    return Err(RpcError::bad_params("realtime priority is not allowed"));
                }
                blocking(move || p.set_process_priority(pid, level)).await?;
                state.audit.record(
                    "process_priority",
                    Some(&ctx.device_id),
                    Some(&ctx.peer),
                    &format!("pid={pid} level={level:?}"),
                );
                Ok(json!({}))
            }
            "profiles.get" => {
                let st = state.clone();
                let profiles = blocking(move || st.p3.profiles().map_err(internal)).await?;
                Ok(json!({"profiles": profiles, "active": state.p3.active_profile()}))
            }
            "profiles.set" => {
                let profiles: Vec<Profile> = typed_param(&params, "profiles")?;
                p3::validate_profiles(&profiles).map_err(RpcError::bad_params)?;
                let st = state.clone();
                blocking(move || {
                    st.p3.set_profiles(profiles).map_err(internal)?;
                    p3::announce_rematch(&st);
                    Ok(())
                })
                .await?;
                Ok(json!({}))
            }
            _ => Err(RpcError::unknown_method(method)),
        }
    }
}

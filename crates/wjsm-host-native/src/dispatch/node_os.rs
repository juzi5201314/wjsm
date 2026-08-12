use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use sysinfo::{Networks, System};
use wjsm_ir::value;
use wjsm_native_abi::NativeVmContext;

use super::{fail_dispatch, modules};
use crate::{NativeAgentState, NativeCallableKind};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum NodeOsMethod {
    Cpus,
    Freemem,
    Homedir,
    Hostname,
    NetworkInterfaces,
    Release,
    Tmpdir,
    Totalmem,
    Type,
    Version,
}

#[derive(Default)]
pub(crate) struct NodeOsState {
    bridge: Option<i64>,
    system: Option<System>,
}

pub(crate) fn ensure_bridge(state: &mut NativeAgentState) -> Option<i64> {
    if let Some(bridge) = state.node_os.bridge {
        return Some(bridge);
    }
    let methods = [
        ("cpus", NodeOsMethod::Cpus),
        ("freemem", NodeOsMethod::Freemem),
        ("homedir", NodeOsMethod::Homedir),
        ("hostname", NodeOsMethod::Hostname),
        ("networkInterfaces", NodeOsMethod::NetworkInterfaces),
        ("release", NodeOsMethod::Release),
        ("tmpdir", NodeOsMethod::Tmpdir),
        ("totalmem", NodeOsMethod::Totalmem),
        ("type", NodeOsMethod::Type),
        ("version", NodeOsMethod::Version),
    ];
    let bridge = state.allocate_object(methods.len() as u32, false).ok()?;
    for (name, method) in methods {
        let callable = state.native_callable(NativeCallableKind::NodeOs(method))?;
        modules::set_named_property(state, bridge, name, callable).ok()?;
    }
    state.node_os.bridge = Some(bridge);
    Some(bridge)
}

pub(crate) fn call(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    method: NodeOsMethod,
) -> i64 {
    let result = match method {
        NodeOsMethod::Cpus => cpu_objects(state),
        NodeOsMethod::Freemem => memory_value(state, false),
        NodeOsMethod::Homedir => string_value(
            state,
            std::env::var_os(home_environment_name())
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_default(),
        ),
        NodeOsMethod::Hostname => string_value(state, System::host_name().unwrap_or_default()),
        NodeOsMethod::NetworkInterfaces => network_interfaces(state),
        NodeOsMethod::Release => string_value(state, System::kernel_version().unwrap_or_default()),
        NodeOsMethod::Tmpdir => {
            string_value(state, std::env::temp_dir().to_string_lossy().into_owned())
        }
        NodeOsMethod::Totalmem => memory_value(state, true),
        NodeOsMethod::Type => string_value(state, System::name().unwrap_or_default()),
        NodeOsMethod::Version => string_value(state, System::long_os_version().unwrap_or_default()),
    };
    result.unwrap_or_else(|| fail_dispatch(ctx))
}

fn system(state: &mut NativeAgentState) -> System {
    state.node_os.system.take().unwrap_or_else(System::new_all)
}

fn memory_value(state: &mut NativeAgentState, total: bool) -> Option<i64> {
    let mut system = system(state);
    system.refresh_memory();
    let bytes = if total {
        system.total_memory()
    } else {
        system.available_memory()
    };
    state.node_os.system = Some(system);
    Some(value::encode_f64(bytes as f64))
}

fn cpu_objects(state: &mut NativeAgentState) -> Option<i64> {
    let mut system = system(state);
    system.refresh_cpu_frequency();
    let cpus = system
        .cpus()
        .iter()
        .map(|cpu| (cpu.brand().to_owned(), cpu.frequency()))
        .collect::<Vec<_>>();
    state.node_os.system = Some(system);

    let mut values = Vec::with_capacity(cpus.len());
    for (model, speed) in cpus {
        let times = object(
            state,
            [
                ("user", value::encode_f64(0.0)),
                ("nice", value::encode_f64(0.0)),
                ("sys", value::encode_f64(0.0)),
                ("idle", value::encode_f64(0.0)),
                ("irq", value::encode_f64(0.0)),
            ],
        )?;
        let model = string_value(state, model)?;
        values.push(object(
            state,
            [
                ("model", model),
                ("speed", value::encode_f64(speed as f64)),
                ("times", times),
            ],
        )?);
    }
    state.allocate_array_values(&values).ok()
}

fn network_interfaces(state: &mut NativeAgentState) -> Option<i64> {
    let networks = Networks::new_with_refreshed_list();
    let mut grouped = BTreeMap::<String, Vec<NetworkAddress>>::new();
    for (name, network) in &networks {
        let mac = network.mac_address().to_string();
        for address in network.ip_networks() {
            grouped
                .entry(name.clone())
                .or_default()
                .push(NetworkAddress::new(
                    address.addr,
                    address.prefix,
                    mac.clone(),
                ));
        }
    }

    let interfaces = state.allocate_object(grouped.len() as u32, false).ok()?;
    for (name, addresses) in grouped {
        let mut values = Vec::with_capacity(addresses.len());
        for address in addresses {
            values.push(address.into_object(state)?);
        }
        let array = state.allocate_array_values(&values).ok()?;
        modules::set_named_property(state, interfaces, &name, array).ok()?;
    }
    Some(interfaces)
}

struct NetworkAddress {
    address: String,
    cidr: String,
    family: &'static str,
    internal: bool,
    mac: String,
    netmask: String,
    scope_id: f64,
}

impl NetworkAddress {
    fn new(address: IpAddr, prefix: u8, mac: String) -> Self {
        let internal = address.is_loopback();
        match address {
            IpAddr::V4(address) => Self {
                address: address.to_string(),
                cidr: format!("{address}/{prefix}"),
                family: "IPv4",
                internal,
                mac,
                netmask: Ipv4Addr::from(prefix_mask_v4(prefix)).to_string(),
                scope_id: 0.0,
            },
            IpAddr::V6(address) => Self {
                address: address.to_string(),
                cidr: format!("{address}/{prefix}"),
                family: "IPv6",
                internal,
                mac,
                netmask: Ipv6Addr::from(prefix_mask_v6(prefix)).to_string(),
                scope_id: 0.0,
            },
        }
    }

    fn into_object(self, state: &mut NativeAgentState) -> Option<i64> {
        let address = string_value(state, self.address)?;
        let netmask = string_value(state, self.netmask)?;
        let family = string_value(state, self.family.into())?;
        let mac = string_value(state, self.mac)?;
        let cidr = string_value(state, self.cidr)?;
        object(
            state,
            [
                ("address", address),
                ("netmask", netmask),
                ("family", family),
                ("mac", mac),
                ("internal", value::encode_bool(self.internal)),
                ("cidr", cidr),
                ("scopeid", value::encode_f64(self.scope_id)),
            ],
        )
    }
}

fn prefix_mask_v4(prefix: u8) -> u32 {
    u32::MAX
        .checked_shl(u32::from(32_u8.saturating_sub(prefix)))
        .unwrap_or(0)
}

fn prefix_mask_v6(prefix: u8) -> u128 {
    u128::MAX
        .checked_shl(u32::from(128_u8.saturating_sub(prefix)))
        .unwrap_or(0)
}

fn object<const N: usize>(
    state: &mut NativeAgentState,
    properties: [(&str, i64); N],
) -> Option<i64> {
    let object = state.allocate_object(N as u32, false).ok()?;
    for (name, stored) in properties {
        modules::set_named_property(state, object, name, stored).ok()?;
    }
    Some(object)
}

fn string_value(state: &mut NativeAgentState, text: String) -> Option<i64> {
    state.intern_text(text, value::TAG_STRING)
}

#[cfg(windows)]
fn home_environment_name() -> &'static str {
    "USERPROFILE"
}

#[cfg(not(windows))]
fn home_environment_name() -> &'static str {
    "HOME"
}

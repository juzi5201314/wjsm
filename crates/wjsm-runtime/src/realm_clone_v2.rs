use anyhow::{Result, bail};
use wjsm_ir::value;

use crate::handle_remap::HandleMap;
use crate::heap::HandleTableV2;
use crate::realm::{Realm, RealmId};

pub fn remap_realm_handles_v2(
    source: &Realm,
    id: RealmId,
    map: &HandleMap,
    handles: &HandleTableV2,
) -> Result<Realm> {
    let global_object = remap_value(source.global_object, map, handles)?;
    let intrinsics = source
        .intrinsics
        .try_map_values(|value| remap_value(value, map, handles))?;
    let mut realm = Realm::new(id, global_object, intrinsics);
    realm.code_generation = source.code_generation;
    realm.microtask_mode = source.microtask_mode;
    Ok(realm)
}

fn remap_value(value: i64, map: &HandleMap, handles: &HandleTableV2) -> Result<i64> {
    if !value::is_object(value) && !value::is_array(value) {
        return Ok(value);
    }
    let source = crate::heap::HandleId::new(value::decode_handle(value));
    if handles.resolve(source).is_none() {
        bail!("V2 realm source handle {} is not live", source.get())
    }
    let target = map.remap_handle_v2(source);
    if handles.resolve(target).is_none() {
        bail!("V2 realm target handle {} is not live", target.get())
    }
    if value::is_object(value) {
        Ok(value::encode_object_handle(target.get()))
    } else {
        Ok(value::encode_handle(value::TAG_ARRAY, target.get()))
    }
}

//! 同宿主 native executable 的预编译 object 编译与编解码。

use std::collections::BTreeMap;
use std::sync::Arc;

use wjsm_artifact_format::PortableArtifact;
use wjsm_backend_native::{CRANELIFT_VERSION, NATIVE_CODEGEN_HASH, NativeCompiler, NativeObject};
use wjsm_exec_format::{EncodedNativeObject, ExecPayload};
use wjsm_ir::Program;
use wjsm_native_abi::native_variable_slots_for_segments;

use crate::{NativeRuntimeError, whole_program_slots};

/// 与 `NativeRuntime::execute` 相同切段规则的预编译 image。
#[derive(Clone, Debug)]
pub enum PrecompiledNativeImages {
    Whole(NativeObject),
    Split {
        builtin: NativeObject,
        user: NativeObject,
    },
}

impl PrecompiledNativeImages {
    pub fn encoded(&self) -> Vec<EncodedNativeObject> {
        match self {
            Self::Whole(object) => vec![encode_object(object)],
            Self::Split { builtin, user } => {
                vec![encode_object(builtin), encode_object(user)]
            }
        }
    }

    pub fn from_encoded(objects: Vec<EncodedNativeObject>) -> Result<Self, NativeRuntimeError> {
        match objects.as_slice() {
            [whole] => Ok(Self::Whole(decode_object(whole)?)),
            [builtin, user] => Ok(Self::Split {
                builtin: decode_object(builtin)?,
                user: decode_object(user)?,
            }),
            _ => Err(NativeRuntimeError::Invariant(
                "native executable must embed 1 or 2 objects".into(),
            )),
        }
    }
}

/// 按 execute 同一套槽号编译 payload 用的 NativeObject。
pub fn compile_native_exec_images(
    artifact: &PortableArtifact,
) -> Result<PrecompiledNativeImages, NativeRuntimeError> {
    let compiler = NativeCompiler::new()?;
    match artifact.program().split_builtin_segment() {
        Some((builtin, user)) => {
            let (builtin_slots, user_slots) = native_variable_slots_for_segments(&builtin, &user);
            Ok(PrecompiledNativeImages::Split {
                builtin: compiler.compile_program_with_slots(&builtin, &builtin_slots)?,
                user: compiler.compile_program_with_slots(&user, &user_slots)?,
            })
        }
        None => {
            let slots = whole_program_slots(artifact.program());
            Ok(PrecompiledNativeImages::Whole(
                compiler.compile_program_with_slots(artifact.program(), &slots)?,
            ))
        }
    }
}

pub fn exec_payload_from_images(
    artifact: &PortableArtifact,
    images: &PrecompiledNativeImages,
    files: BTreeMap<String, Vec<u8>>,
) -> Result<ExecPayload, NativeRuntimeError> {
    let compiler = NativeCompiler::new()?;
    Ok(ExecPayload {
        native_abi_hash: wjsm_native_abi::native_abi_hash(),
        codegen_hash: NATIVE_CODEGEN_HASH,
        target: ExecPayload::host_target(),
        cranelift_version: CRANELIFT_VERSION.to_owned(),
        settings: compiler.settings_key().to_owned(),
        files,
        artifact: artifact.bytes().to_vec(),
        objects: images.encoded(),
    })
}

pub fn images_from_exec_payload(
    payload: &ExecPayload,
) -> Result<(Arc<PortableArtifact>, PrecompiledNativeImages), NativeRuntimeError> {
    payload
        .verify_stub_identity(
            wjsm_native_abi::native_abi_hash(),
            NATIVE_CODEGEN_HASH,
            CRANELIFT_VERSION,
        )
        .map_err(|error| NativeRuntimeError::Invariant(error.to_string()))?;
    let artifact = PortableArtifact::decode(
        Arc::<[u8]>::from(payload.artifact.clone()),
        &wjsm_artifact_format::ArtifactLimits::default(),
    )
    .map_err(|error| NativeRuntimeError::Artifact(error.to_string()))?;
    let images = PrecompiledNativeImages::from_encoded(payload.objects.clone())?;
    validate_images_match_program(artifact.program(), &images)?;
    Ok((Arc::new(artifact), images))
}

pub(crate) fn validate_images_match_program(
    program: &Program,
    images: &PrecompiledNativeImages,
) -> Result<(), NativeRuntimeError> {
    match (program.split_builtin_segment(), images) {
        (Some(_), PrecompiledNativeImages::Split { .. }) => Ok(()),
        (None, PrecompiledNativeImages::Whole(_)) => Ok(()),
        (Some(_), PrecompiledNativeImages::Whole(_)) => Err(NativeRuntimeError::Invariant(
            "precompiled executable is missing the builtin image segment".into(),
        )),
        (None, PrecompiledNativeImages::Split { .. }) => Err(NativeRuntimeError::Invariant(
            "precompiled executable has a builtin image but the program is not split".into(),
        )),
    }
}

fn encode_object(object: &NativeObject) -> EncodedNativeObject {
    EncodedNativeObject {
        bytes: object.bytes().to_vec(),
        frame_bytes: object.frame_bytes().to_vec(),
        function_count: object.function_count(),
        ic_slot_count: object.ic_slot_count(),
        feedback_slot_count: object.feedback_slot_count(),
    }
}

fn decode_object(object: &EncodedNativeObject) -> Result<NativeObject, NativeRuntimeError> {
    Ok(NativeObject::from_parts(
        object.bytes.clone(),
        object.frame_bytes.clone(),
        object.function_count,
        object.ic_slot_count,
        object.feedback_slot_count,
    )?)
}

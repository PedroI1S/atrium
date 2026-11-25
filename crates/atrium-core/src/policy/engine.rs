//! `DefaultPolicy`: pure rev 4 policy engine (`PolicyEngine` implementation).
//!
//! Never touches the filesystem (contract §5): `SessionGate` evaluates with
//! `v4::paths::FsResolver` and `build_protected_set`, while `DefaultPolicy`
//! evaluates lexically over a pure [`VirtualResolver`].

use std::path::Path;

use atrium_protocol::policy::Rule;

use super::DefaultPolicy;
use super::model::{
    ComputeBudget, DirEntry, FoldRules, FsCaps, PathMetadata, ProtectedSet, ResolveError,
};
use super::v4;
use crate::{PolicyEngine, PolicyInput, PolicyVerdict};

/// Pure, non-filesystem [`v4::paths::PathResolver`] used by
/// [`DefaultPolicy::evaluate`] (contract §5).
struct VirtualResolver;

impl v4::paths::PathResolver for VirtualResolver {
    fn stat(
        &self,
        _path: &[u8],
        caps: &mut FsCaps,
        budget: &mut ComputeBudget,
    ) -> Result<PathMetadata, ResolveError> {
        caps.charge_stat()?;
        budget.charge(1)?;
        Err(ResolveError::NotFound)
    }

    fn lstat(
        &self,
        _path: &[u8],
        caps: &mut FsCaps,
        budget: &mut ComputeBudget,
    ) -> Result<PathMetadata, ResolveError> {
        caps.charge_stat()?;
        budget.charge(1)?;
        Err(ResolveError::NotFound)
    }

    fn read_link(
        &self,
        _path: &[u8],
        caps: &mut FsCaps,
        budget: &mut ComputeBudget,
    ) -> Result<Vec<u8>, ResolveError> {
        caps.charge_link_read()?;
        budget.charge(1)?;
        Err(ResolveError::NotFound)
    }

    fn canonicalize(
        &self,
        _path: &[u8],
        caps: &mut FsCaps,
        budget: &mut ComputeBudget,
    ) -> Result<Vec<u8>, ResolveError> {
        caps.charge_canonicalize()?;
        budget.charge(1)?;
        Err(ResolveError::NotFound)
    }

    fn read_dir(
        &self,
        _path: &[u8],
        caps: &mut FsCaps,
        budget: &mut ComputeBudget,
    ) -> Result<Vec<DirEntry>, ResolveError> {
        caps.charge_dir_open()?;
        budget.charge(1)?;
        Err(ResolveError::NotFound)
    }
}

#[cfg(unix)]
fn engine_path_bytes(p: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    p.as_os_str().as_bytes().to_vec()
}

#[cfg(not(unix))]
fn engine_path_bytes(p: &Path) -> Vec<u8> {
    p.as_os_str().as_encoded_bytes().to_vec()
}

impl PolicyEngine for DefaultPolicy {
    fn evaluate(&self, input: &PolicyInput<'_>, rules: &[Rule]) -> PolicyVerdict {
        let protected = ProtectedSet {
            home: engine_path_bytes(input.home),
            cwd: engine_path_bytes(input.cwd),
            atrium_data_paths: input
                .atrium_data_paths
                .iter()
                .map(|p| engine_path_bytes(p))
                .collect(),
            atrium_protected_paths: input
                .atrium_protected_paths
                .iter()
                .map(|p| engine_path_bytes(p))
                .collect(),
            hard_deny_identities: Vec::new(),
            write_only_hard_deny_identities: Vec::new(),
            identity_gaps: Vec::new(),
            store_identities: Vec::new(),
            ssh_linked_dirs: Vec::new(),
            folds: FoldRules::REV4,
        };
        v4::evaluate::evaluate_call(input, rules, &protected, &VirtualResolver).verdict
    }
}

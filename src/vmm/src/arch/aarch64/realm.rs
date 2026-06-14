// Copyright 2025 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! ARM CCA (Confidential Compute Architecture) RME (Realm Management Extension)
//! support for Firecracker microVMs.
//!
//! This module provides FFI bindings for the KVM RME ABI and a [`RealmManager`]
//! that orchestrates the realm lifecycle: configuration, creation, memory
//! population, and activation.
//!
//! The KVM RME interface uses `KVM_ENABLE_CAP` with `KVM_CAP_ARM_RME` and
//! sub-operations passed via the `args` array of [`kvm_bindings::kvm_enable_cap`].

use kvm_bindings::kvm_enable_cap;
use kvm_ioctls::{VcpuFd, VmFd};
use vmm_sys_util::ioctl::ioctl_with_ref;

// ---------------------------------------------------------------------------
// KVM RME ABI constants (kernel v10, Aug 2025)
// ---------------------------------------------------------------------------

/// KVM capability number for ARM Realm Management Extension.
///
/// Gates all RME functionality. Checked via `KVM_CHECK_EXTENSION` on the
/// system (KVM) fd. Returns 0 if unsupported, positive otherwise.
pub const KVM_CAP_ARM_RME: u32 = 243;

/// VM type flag passed to `KVM_CREATE_VM` to create a Realm VM.
///
/// Must be OR'd into the `type` argument of `KVM_CREATE_VM`. Cannot be
/// changed after VM creation.
pub const KVM_VM_TYPE_ARM_REALM: u64 = 1 << 8;

/// vCPU feature bit that initializes a vCPU as a Realm Execution Context (REC).
///
/// Set in `kvm_vcpu_init::features[0]` before `KVM_ARM_VCPU_INIT`, then
/// finalized via `KVM_ARM_VCPU_FINALIZE` after boot registers are configured.
pub const KVM_ARM_VCPU_REC: u32 = 8;

// -- Sub-operations for KVM_ENABLE_CAP with KVM_CAP_ARM_RME ----------------

/// Sub-op: configure realm parameters (RPV and measurement hash algorithm).
///
/// `args[0]` = this value, `args[1]` = pointer to [`ArmRmeConfig`].
pub const KVM_CAP_ARM_RME_CONFIG_REALM: u64 = 0;

/// Sub-op: create the Realm Descriptor (RD) in KVM/RMM.
///
/// `args[0]` = this value. No additional arguments.
pub const KVM_CAP_ARM_RME_CREATE_REALM: u64 = 1;

/// Sub-op: declare the Realm IPA Space (RIPAS) RAM ranges.
///
/// `args[0]` = this value, `args[1]` = pointer to [`ArmRmeInitRipas`].
pub const KVM_CAP_ARM_RME_INIT_RIPAS_REALM: u64 = 2;

/// Sub-op: populate realm memory pages and optionally measure them.
///
/// `args[0]` = this value, `args[1]` = pointer to [`ArmRmePopulateRealm`].
pub const KVM_CAP_ARM_RME_POPULATE_REALM: u64 = 3;

/// Sub-op: seal and activate the realm, transitioning to running state.
///
/// `args[0]` = this value. No additional arguments.
/// After this call, the VMM can no longer modify realm memory.
pub const KVM_CAP_ARM_RME_ACTIVATE_REALM: u64 = 4;

// -- ArmRmeConfig::cfg field values -----------------------------------------

/// Config selector: set the Realm Personalization Value (RPV).
///
/// The RPV is a 64-byte blob that binds the realm to a specific identity.
/// It is mixed into the Realm Initial Measurement (RIM).
pub const ARM_RME_CONFIG_RPV: u32 = 0;

/// Config selector: set the measurement hash algorithm.
pub const ARM_RME_CONFIG_HASH_ALGO: u32 = 1;

// -- Measurement algorithm values for ArmRmeConfig::hash_algo ---------------

/// SHA-256 measurement algorithm.
pub const ARM_RME_MEASUREMENT_ALGO_SHA256: u32 = 0;

/// SHA-512 measurement algorithm.
pub const ARM_RME_MEASUREMENT_ALGO_SHA512: u32 = 1;

// -- Populate flags ---------------------------------------------------------

/// When set in [`ArmRmePopulateRealm::flags`], the populated pages are
/// included in the realm measurement (RIM).
pub const KVM_ARM_RME_POPULATE_FLAGS_MEASURE: u32 = 1 << 0;

/// Size of the Realm Personalization Value in bytes.
pub const ARM_RME_CONFIG_RPV_SIZE: usize = 64;

// ---------------------------------------------------------------------------
// KVM RME ABI structures
// ---------------------------------------------------------------------------

/// Union inside [`ArmRmeConfig`] holding either the RPV or the hash algorithm.
///
/// The kernel reads only the field indicated by [`ArmRmeConfig::cfg`].
/// The union is 64 bytes (size of the largest member, `rpv`).
#[repr(C)]
pub union ArmRmeConfigData {
    /// Realm Personalization Value — 64 bytes mixed into the RIM.
    pub rpv: [u8; ARM_RME_CONFIG_RPV_SIZE],
    /// Measurement hash algorithm selector.
    pub hash_algo: u32,
}

impl Default for ArmRmeConfigData {
    fn default() -> Self {
        Self {
            rpv: [0u8; ARM_RME_CONFIG_RPV_SIZE],
        }
    }
}

/// Configuration structure for `KVM_CAP_ARM_RME_CONFIG_REALM`.
///
/// Matches the kernel `struct arm_rme_config` ABI. The `cfg` field selects
/// which union member the kernel reads: [`ARM_RME_CONFIG_RPV`] reads `rpv`,
/// [`ARM_RME_CONFIG_HASH_ALGO`] reads `hash_algo`.
#[repr(C)]
pub struct ArmRmeConfig {
    /// Selector: [`ARM_RME_CONFIG_RPV`] or [`ARM_RME_CONFIG_HASH_ALGO`].
    pub cfg: u32,
    /// Reserved, must be zero.
    pub reserved: u32,
    /// Union of configuration payloads.
    pub data: ArmRmeConfigData,
}

impl Default for ArmRmeConfig {
    fn default() -> Self {
        Self {
            cfg: 0,
            reserved: 0,
            data: ArmRmeConfigData::default(),
        }
    }
}

/// RIPAS (Realm IPA Space) initialization structure for
/// `KVM_CAP_ARM_RME_INIT_RIPAS_REALM`.
///
/// Matches the kernel `struct arm_rme_init_ripas` ABI. Declares a contiguous
/// range of guest physical addresses as RAM within the realm.
#[repr(C)]
#[derive(Debug, Default, Clone)]
pub struct ArmRmeInitRipas {
    /// Base guest physical address (must be page-aligned).
    pub base: u64,
    /// Size of the region in bytes (must be page-aligned).
    pub size: u64,
    /// Reserved, must be zero.
    pub reserved: [u64; 2],
}

/// Realm population structure for `KVM_CAP_ARM_RME_POPULATE_REALM`.
///
/// Matches the kernel `struct arm_rme_populate_realm` ABI. Populates a range
/// of guest physical pages and optionally includes them in the measurement.
#[repr(C)]
#[derive(Debug, Default, Clone)]
pub struct ArmRmePopulateRealm {
    /// Base guest physical address (must be page-aligned).
    pub base: u64,
    /// Size of the region in bytes (must be page-aligned).
    pub size: u64,
    /// Bitmask of populate flags. Set [`KVM_ARM_RME_POPULATE_FLAGS_MEASURE`]
    /// to include these pages in the Realm Initial Measurement.
    pub flags: u32,
    /// Reserved, must be zero.
    pub reserved: [u32; 3],
}

// ---------------------------------------------------------------------------
// Rust-level types
// ---------------------------------------------------------------------------

/// Measurement hash algorithm for the Realm Initial Measurement (RIM).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum MeasurementAlgo {
    /// SHA-256
    Sha256 = ARM_RME_MEASUREMENT_ALGO_SHA256,
    /// SHA-512
    Sha512 = ARM_RME_MEASUREMENT_ALGO_SHA512,
}

/// Rust-level configuration for a Realm VM.
///
/// Provided at [`RealmManager`] construction time and used during the
/// [`RealmManager::configure`] step.
#[derive(Debug, Clone)]
pub struct RealmConfig {
    /// Hash algorithm used for the Realm Initial Measurement.
    pub measurement_algo: MeasurementAlgo,
    /// Optional 64-byte Realm Personalization Value. When `Some`, the RPV
    /// is mixed into the RIM, binding the realm to a specific identity.
    pub personalization_value: Option<[u8; ARM_RME_CONFIG_RPV_SIZE]>,
}

/// Errors produced by [`RealmManager`] operations.
#[derive(Debug, thiserror::Error, displaydoc::Display)]
pub enum RealmError {
    /// Failed to configure realm (RPV or hash algorithm): {0}
    Configure(kvm_ioctls::Error),
    /// Failed to create realm descriptor: {0}
    Create(kvm_ioctls::Error),
    /// Failed to initialize RIPAS for memory region: {0}
    InitRipas(kvm_ioctls::Error),
    /// Failed to populate realm memory: {0}
    Populate(kvm_ioctls::Error),
    /// Failed to activate realm: {0}
    Activate(kvm_ioctls::Error),
    /// Failed to finalize vCPU as Realm Execution Context: {0}
    FinalizeVcpu(kvm_ioctls::Error),
}

// ---------------------------------------------------------------------------
// RealmManager
// ---------------------------------------------------------------------------

/// Manages the ARM CCA RME realm lifecycle for a single microVM.
///
/// The typical call sequence is:
///
/// 1. [`RealmManager::is_supported`] — check KVM support
/// 2. [`RealmManager::vm_type`] — pass to `KVM_CREATE_VM`
/// 3. [`RealmManager::configure`] — set RPV and hash algorithm
/// 4. [`RealmManager::create_realm`] — create the Realm Descriptor
/// 5. [`RealmManager::init_ripas`] — declare RAM ranges
/// 6. [`RealmManager::populate`] — load images and measure pages
/// 7. [`RealmManager::finalize_vcpu`] — finalize each REC
/// 8. [`RealmManager::activate`] — seal and start the realm
#[derive(Debug)]
pub struct RealmManager {
    config: RealmConfig,
}

impl RealmManager {
    /// Create a new [`RealmManager`] with the given configuration.
    pub fn new(config: RealmConfig) -> Self {
        Self { config }
    }

    /// Check whether the host KVM supports ARM RME.
    ///
    /// Returns `true` if `KVM_CAP_ARM_RME` is available on the system KVM fd.
    pub fn is_supported(kvm_fd: &kvm_ioctls::Kvm) -> bool {
        kvm_fd.check_extension_raw(u64::from(KVM_CAP_ARM_RME)) != 0
    }

    /// Return the VM type flag for realm creation.
    ///
    /// This value must be OR'd into the `type` argument of `KVM_CREATE_VM`.
    pub fn vm_type(&self) -> u64 {
        KVM_VM_TYPE_ARM_REALM
    }

    /// Configure the realm's security parameters (hash algorithm and optional RPV).
    ///
    /// Issues one or two `KVM_ENABLE_CAP` calls with
    /// `KVM_CAP_ARM_RME_CONFIG_REALM`:
    /// - Always sets the measurement hash algorithm.
    /// - If a personalization value is configured, sets the RPV.
    pub fn configure(&self, vm_fd: &VmFd) -> Result<(), RealmError> {
        // Set measurement hash algorithm.
        let algo_config = ArmRmeConfig {
            cfg: ARM_RME_CONFIG_HASH_ALGO,
            reserved: 0,
            data: ArmRmeConfigData {
                hash_algo: self.config.measurement_algo as u32,
            },
        };
        let cap = Self::build_cap(KVM_CAP_ARM_RME_CONFIG_REALM, &algo_config);
        Self::kvm_enable_cap_ioctl(vm_fd, &cap).map_err(RealmError::Configure)?;

        // Set RPV if provided.
        if let Some(rpv) = &self.config.personalization_value {
            let rpv_config = ArmRmeConfig {
                cfg: ARM_RME_CONFIG_RPV,
                reserved: 0,
                data: ArmRmeConfigData { rpv: *rpv },
            };
            let cap = Self::build_cap(KVM_CAP_ARM_RME_CONFIG_REALM, &rpv_config);
Self::kvm_enable_cap_ioctl(vm_fd, &cap).map_err(RealmError::Configure)?;
        }

        Ok(())
    }

    /// Create the Realm Descriptor (RD) in KVM/RMM.
    ///
    /// This must be called after [`configure`](Self::configure) and before
    /// any memory operations.
    pub fn create_realm(&self, vm_fd: &VmFd) -> Result<(), RealmError> {
        let cap = kvm_enable_cap {
            cap: KVM_CAP_ARM_RME,
            flags: 0,
            args: [KVM_CAP_ARM_RME_CREATE_REALM, 0, 0, 0],
            pad: [0; 64],
        };
        Self::kvm_enable_cap_ioctl(vm_fd, &cap).map_err(RealmError::Create)
    }

    /// Declare a RAM region in the Realm IPA Space (RIPAS).
    ///
    /// Both `base` and `size` must be page-aligned. This tells the RMM which
    /// guest physical address ranges will be used as realm RAM.
    pub fn init_ripas(
        &self,
        vm_fd: &VmFd,
        base: u64,
        size: u64,
    ) -> Result<(), RealmError> {
        let ripas = ArmRmeInitRipas {
            base,
            size,
            reserved: [0; 2],
        };
        let cap = Self::build_cap(KVM_CAP_ARM_RME_INIT_RIPAS_REALM, &ripas);
        Self::kvm_enable_cap_ioctl(vm_fd, &cap).map_err(RealmError::InitRipas)
    }

    /// Populate a range of guest physical pages and optionally measure them.
    ///
    /// When `measure` is `true`, the [`KVM_ARM_RME_POPULATE_FLAGS_MEASURE`]
    /// flag is set, causing the page contents to be included in the Realm
    /// Initial Measurement (RIM). Both `base` and `size` must be page-aligned.
    pub fn populate(
        &self,
        vm_fd: &VmFd,
        base: u64,
        size: u64,
        measure: bool,
    ) -> Result<(), RealmError> {
        let flags = if measure {
            KVM_ARM_RME_POPULATE_FLAGS_MEASURE
        } else {
            0
        };
        let pop = ArmRmePopulateRealm {
            base,
            size,
            flags,
            reserved: [0; 3],
        };
        let cap = Self::build_cap(KVM_CAP_ARM_RME_POPULATE_REALM, &pop);
        Self::kvm_enable_cap_ioctl(vm_fd, &cap).map_err(RealmError::Populate)
    }

    /// Activate the realm, sealing it and transitioning to running state.
    ///
    /// After activation, the VMM can no longer modify realm memory or
    /// configuration. All images must be loaded and all RECs finalized
    /// before calling this.
    pub fn activate(&self, vm_fd: &VmFd) -> Result<(), RealmError> {
        let cap = kvm_enable_cap {
            cap: KVM_CAP_ARM_RME,
            flags: 0,
            args: [KVM_CAP_ARM_RME_ACTIVATE_REALM, 0, 0, 0],
            pad: [0; 64],
        };
        Self::kvm_enable_cap_ioctl(vm_fd, &cap).map_err(RealmError::Activate)
    }

    /// Finalize a vCPU as a Realm Execution Context (REC).
    ///
    /// Must be called after the vCPU's boot registers are configured and
    /// before [`activate`](Self::activate).
    pub fn finalize_vcpu(&self, vcpu_fd: &VcpuFd) -> Result<(), RealmError> {
        // KVM_ARM_VCPU_REC has value 8, which fits in i32 without overflow.
        #[allow(clippy::cast_possible_wrap)]
        let feature = KVM_ARM_VCPU_REC as i32;
        vcpu_fd
            .vcpu_finalize(&feature)
            .map_err(RealmError::FinalizeVcpu)
    }

    /// Ioctl number for `KVM_ENABLE_CAP`.
    ///
    /// Computed from `_IOW(0xAE, 0xa3, struct kvm_enable_cap)`:
    /// `(1u64 << 30) | (104u64 << 16) | (0xAEu64 << 8) | 0xa3u64` = `0x4068_AEA3`.
    #[allow(clippy::cast_possible_truncation)]
    const KVM_ENABLE_CAP_IOCTL: u64 =
        (1u64 << 30) | (104u64 << 16) | (0xAEu64 << 8) | 0xa3u64;

    /// Issue a `KVM_ENABLE_CAP` ioctl.
    ///
    /// `VmFd::enable_cap()` is not available on aarch64 in kvm_ioctls 0.24.0,
    /// so we invoke the ioctl directly via `ioctl_with_ref`.
    fn kvm_enable_cap_ioctl(vm_fd: &VmFd, cap: &kvm_enable_cap) -> Result<(), kvm_ioctls::Error> {
        // SAFETY: `vm_fd` is a valid KVM VM file descriptor. `cap` is a valid
        // `kvm_enable_cap` struct with `#[repr(C)]` layout matching the kernel ABI.
        // The kernel reads the struct but does not retain the pointer after the call.
        let ret = unsafe { ioctl_with_ref(vm_fd, Self::KVM_ENABLE_CAP_IOCTL, cap) };
        if ret == 0 {
            Ok(())
        } else {
            Err(vmm_sys_util::errno::Error::last())
        }
    }

    /// Build a [`kvm_enable_cap`] for `KVM_CAP_ARM_RME` with a sub-operation
    /// and a pointer to the argument struct.
    ///
    /// `args[0]` = sub-op, `args[1]` = pointer to `arg`.
    fn build_cap<T>(sub_op: u64, arg: &T) -> kvm_enable_cap {
        // SAFETY: We create a raw pointer from a valid reference that lives
        // for the entire duration of the enclosing `kvm_enable_cap_ioctl` call.
        // The kernel reads exactly `size_of::<T>()` bytes from this address,
        // and the `#[repr(C)]` layout guarantees ABI compatibility.
        let ptr = arg as *const T as u64;
        kvm_enable_cap {
            cap: KVM_CAP_ARM_RME,
            flags: 0,
            args: [sub_op, ptr, 0, 0],
            pad: [0; 64],
        }
    }
}

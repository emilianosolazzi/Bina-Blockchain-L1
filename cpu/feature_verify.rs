use std::collections::HashSet;
use super::{CpuFeature, CpuDetectionError};
use raw_cpuid::CpuId;
use tracing::{debug, warn};

#[inline(always)]
pub(crate) fn verify_htt_support(cpuid: &CpuId) -> Result<bool, CpuDetectionError> {
    // Multi-platform HTT verification
    let mut htt_status = false;

    // 1. Check CPUID feature flag first
    if let Some(fi) = cpuid.get_feature_info() {
        if !fi.has_htt() {
            return Ok(false);
        }
        htt_status = true;
    }

    // 2. Platform-specific verification
    #[cfg(target_os = "linux")]
    {
        // Check sysfs if available
        match std::fs::read_to_string("/sys/devices/system/cpu/smt/active") {
            Ok(cores) => {
                htt_status = cores.trim() == "1";
            },
            Err(e) => {
                debug!("Failed to read SMT status from sysfs: {}", e);
                // Fall through to other checks
            }
        }

        // Additional check via topology
        if let Some(topo) = cpuid.get_topology_info() {
            htt_status = topo.num_threads() > topo.num_cores();
        }
    }

    // 3. Verify via processor capacity info
    if let Some(pi) = cpuid.get_processor_capacity_info() {
        if let (Some(phys), Some(log)) = (pi.physical_cores(), pi.logical_cores()) {
            htt_status = log > phys;
        }
    }

    Ok(htt_status)
}

pub(crate) fn verify_avx_support(
    cpuid: &CpuId,
    features: &mut HashSet<CpuFeature>
) -> Result<(), CpuDetectionError> {
    // Check for required OSXSAVE feature first
    if let Some(fi) = cpuid.get_feature_info() {
        if !fi.has_osxsave() {
            features.remove(&CpuFeature::AVX);
            features.remove(&CpuFeature::AVX2);
            features.remove(&CpuFeature::FMA);
            return Ok(());
        }
    }

    // Architecture-specific verification
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        let xcr0 = match unsafe { get_xcr0() } {
            Ok(val) => val,
            Err(_) => {
                // If we can't read XCR0, assume no AVX support
                features.remove(&CpuFeature::AVX);
                features.remove(&CpuFeature::AVX2);
                features.remove(&CpuFeature::FMA);
                return Ok(());
            }
        };

        // Check bits 1 (SSE) and 2 (AVX)
        if (xcr0 & 0b110) != 0b110 {
            features.remove(&CpuFeature::AVX);
            features.remove(&CpuFeature::AVX2);
            features.remove(&CpuFeature::FMA);
        }
    }

    #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
    {
        // Non-x86 platforms don't support AVX
        features.remove(&CpuFeature::AVX);
        features.remove(&CpuFeature::AVX2);
        features.remove(&CpuFeature::FMA);
    }

    Ok(())
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
unsafe fn get_xcr0() -> Result<u64, CpuDetectionError> {
    let (eax, edx): (u32, u32);
    std::arch::asm!(
        "xor ecx, ecx",
        "xgetbv",
        out("eax") eax,
        out("edx") edx,
        options(nomem, nostack)
    );
    Ok((edx as u64) << 32 | (eax as u64))
}

pub(crate) fn harmonize_features(
    cpuid: &CpuId,
    features: &mut HashSet<CpuFeature>,
) -> Result<(), CpuDetectionError> {
    // Validate feature dependencies
    if features.contains(&CpuFeature::AVX2) && !features.contains(&CpuFeature::AVX) {
        warn!("Invalid feature combination: AVX2 without AVX");
        features.remove(&CpuFeature::AVX2);
    }

    if features.contains(&CpuFeature::FMA) && !features.contains(&CpuFeature::AVX) {
        warn!("Invalid feature combination: FMA without AVX");
        features.remove(&CpuFeature::FMA);
    }

    // Check processor capabilities
    if let Some(pi) = cpuid.get_processor_capacity_info() {
        // Adjust features based on core count
        if let Some(cores) = pi.physical_cores() {
            if cores == 1 {
                features.remove(&CpuFeature::HTT);
            }
        }

        // Validate SGX support
        if features.contains(&CpuFeature::SGX) {
            if !pi.sgx_support().unwrap_or(false) {
                features.remove(&CpuFeature::SGX);
            }
        }
    }

    // Ensure required features for protocol
    if features.contains(&CpuFeature::RTM) && !features.contains(&CpuFeature::HLE) {
        features.remove(&CpuFeature::RTM);
    }

    Ok(())
}

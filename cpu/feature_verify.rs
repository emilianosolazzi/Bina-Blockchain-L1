use std::collections::HashSet;
use super::{CpuFeature, CpuDetectionError};

#[inline(always)]
pub(crate) fn verify_htt_support() -> bool {
    #[cfg(target_os = "linux")]
    {
        // Check sysfs for actual HTT status
        if let Ok(cores) = std::fs::read_to_string("/sys/devices/system/cpu/smt/active") {
            return cores.trim() == "1";
        }
    }
    true
}

pub(crate) fn verify_avx_support(features: &mut HashSet<CpuFeature>) -> Result<(), CpuDetectionError> {
    // Check XCR0 register for AVX state support
    unsafe {
        let xcr0: u64;
        #[cfg(target_arch = "x86_64")]
        std::arch::asm!(
            "xgetbv",
            in("ecx") 0,
            out("eax") xcr0,
            out("edx") _,
        );

        // Verify both YMM and XMM states are enabled
        if (xcr0 & 0b110) != 0b110 {
            features.remove(&CpuFeature::AVX);
            features.remove(&CpuFeature::AVX2);
        }
    }
    Ok(())
}

pub(crate) fn harmonize_features(
    features: &mut HashSet<CpuFeature>,
    pi: raw_cpuid::ProcessorCapacityInfo
) -> Result<(), CpuDetectionError> {
    // Ensure feature combinations are valid for the protocol
    if features.contains(&CpuFeature::AVX2) && !features.contains(&CpuFeature::AVX) {
        features.remove(&CpuFeature::AVX2);
    }

    // Adjust features based on processor capacity
    if let Some(cores) = pi.physical_cores() {
        if cores == 1 {
            features.remove(&CpuFeature::HTT);
        }
    }

    Ok(())
}

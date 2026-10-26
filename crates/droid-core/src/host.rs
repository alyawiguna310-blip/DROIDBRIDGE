//! Read-only report of host capabilities visible to the current core process.
//!
//! This module deliberately reports host/build facts separately from
//! Android-facing [`crate::DeviceProfile`] identity. It does not spoof device
//! properties or claim support for CPU instructions, GPU features, or graphics
//! APIs that have not been detected.

/// Basic host information available without platform-specific dependencies.
///
/// The OS and architecture are Rust's compile-time target constants. In a
/// native build they describe the host platform; in a cross-compiled build
/// they describe the target platform. The logical CPU count is queried at
/// runtime and may be lower than the machine's physical/logical CPU total due
/// to affinity, containers, or process limits.
///
/// RAM, GPU details, SIMD extensions, and graphics API support are intentionally
/// not inferred here. Those require dedicated, platform-aware probes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostCapabilities {
    /// Operating-system target name reported by std::env::consts::OS.
    pub operating_system: &'static str,
    /// CPU architecture target name reported by std::env::consts::ARCH.
    pub architecture: &'static str,
    /// Logical CPUs available to the current process, when the OS query works.
    pub available_logical_cpus: Option<usize>,
}

impl HostCapabilities {
    /// Query the host-process information that Rust's standard library exposes.
    ///
    /// Failure to query the available logical CPU count is represented as
    /// None; it does not make the whole host report unavailable.
    pub fn detect() -> Self {
        Self {
            operating_system: std::env::consts::OS,
            architecture: std::env::consts::ARCH,
            available_logical_cpus: std::thread::available_parallelism()
                .ok()
                .map(|count| count.get()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_the_build_target_without_inventing_hardware_features() {
        let host = HostCapabilities::detect();

        assert_eq!(host.operating_system, std::env::consts::OS);
        assert_eq!(host.architecture, std::env::consts::ARCH);
    }

    #[test]
    fn available_logical_cpu_count_is_nonzero_when_reported() {
        let host = HostCapabilities::detect();

        assert!(
            host.available_logical_cpus.is_none_or(|count| count > 0),
            "a reported logical CPU count must be nonzero"
        );
    }

    #[test]
    fn detection_is_repeatable_for_build_target_fields() {
        let first = HostCapabilities::detect();
        let second = HostCapabilities::detect();

        assert_eq!(first.operating_system, second.operating_system);
        assert_eq!(first.architecture, second.architecture);
    }
}

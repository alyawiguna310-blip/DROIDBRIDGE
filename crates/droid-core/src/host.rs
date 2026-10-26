//! Read-only report of host capabilities visible to the current core process.

use sysinfo::System;
//!
//! This module deliberately reports host/build facts separately from
//! Android-facing [`crate::DeviceProfile`] identity. It does not spoof device
//! properties or claim support for CPU instructions, GPU features, or graphics
//! APIs that have not been detected.

/// Basic host information collected through portable Rust APIs and sysinfo.
///
/// The OS and architecture are Rust's compile-time target constants. In a
/// native build they describe the host platform; in a cross-compiled build
/// they describe the target platform. The logical CPU count is queried at
/// runtime and may be lower than the machine's physical/logical CPU total due
/// to affinity, containers, or process limits.
///
/// GPU details, SIMD extensions, and graphics API support are intentionally not
/// inferred here. Those require dedicated, platform-aware probes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostCapabilities {
    /// Operating-system target name reported by std::env::consts::OS.
    pub operating_system: &'static str,
    /// CPU architecture target name reported by std::env::consts::ARCH.
    pub architecture: &'static str,
    /// Logical CPUs available to the current process, when the OS query works.
    pub available_logical_cpus: Option<usize>,
    /// Total physical memory reported by the operating system, in bytes.
    pub total_memory_bytes: Option<u64>,
    /// Memory the operating system currently considers available, in bytes.
    pub available_memory_bytes: Option<u64>,
}

impl HostCapabilities {
    /// Query basic host-process and system-memory information.
    ///
    /// Failure to query the logical CPU count or memory values is represented
    /// as `None`; it does not make the whole host report unavailable. Memory
    /// fields are populated only when the system reports a nonzero total.
    pub fn detect() -> Self {
        let mut system = System::new();
        system.refresh_memory();

        let total_memory = system.total_memory();
        let (total_memory_bytes, available_memory_bytes) = if total_memory > 0 {
            (Some(total_memory), Some(system.available_memory()))
        } else {
            (None, None)
        };

        Self {
            operating_system: std::env::consts::OS,
            architecture: std::env::consts::ARCH,
            available_logical_cpus: std::thread::available_parallelism()
                .ok()
                .map(|count| count.get()),
            total_memory_bytes,
            available_memory_bytes,
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
    fn memory_values_are_consistent_when_reported() {
        let host = HostCapabilities::detect();

        match (host.total_memory_bytes, host.available_memory_bytes) {
            (Some(total), Some(available)) => {
                assert!(total > 0);
                assert!(available <= total);
            }
            (None, None) => {}
            _ => panic!("total and available memory must be reported together"),
        }
    }

    #[test]
    fn detection_is_repeatable_for_build_target_fields() {
        let first = HostCapabilities::detect();
        let second = HostCapabilities::detect();

        assert_eq!(first.operating_system, second.operating_system);
        assert_eq!(first.architecture, second.architecture);
    }
}

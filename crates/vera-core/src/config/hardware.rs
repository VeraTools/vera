//! Platform memory detection and stable host/device fingerprints.

use super::OnnxExecutionProvider;

/// GPU information collected from a single detection pass.
#[derive(Debug, Clone)]
pub struct GpuInfo {
    /// Free VRAM in MB, if detectable.
    pub vram_free_mb: Option<u64>,
    /// Device fingerprint string for profile keying.
    pub fingerprint: String,
}

/// Detect GPU information (VRAM and device fingerprint) for the given
/// execution provider. Runs the vendor CLI tool once and extracts both
/// pieces of data, avoiding duplicate subprocess calls.
pub fn detect_gpu_info(ep: OnnxExecutionProvider) -> GpuInfo {
    match ep {
        OnnxExecutionProvider::Cuda => detect_nvidia_gpu_info(),
        OnnxExecutionProvider::Rocm => detect_rocm_gpu_info(),
        OnnxExecutionProvider::CoreMl => detect_apple_silicon_mem_info(),
        _ => GpuInfo {
            vram_free_mb: None,
            fingerprint: host_fingerprint(ep),
        },
    }
}

/// Apple Silicon uses unified memory: the GPU shares system RAM. Report half
/// of total RAM (via `sysctl hw.memsize`) as the available pool so batch
/// auto-scaling works; returns None for VRAM off-macOS or on parse failure.
fn detect_apple_silicon_mem_info() -> GpuInfo {
    let output = std::process::Command::new("sysctl")
        .args(["-n", "hw.memsize"])
        .output()
        .ok();
    let total_mb = output
        .filter(|o| o.status.success())
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .trim()
                .parse::<u64>()
                .ok()
        })
        .map(|bytes| bytes / (1024 * 1024));
    // Half of system RAM is a conservative proxy for what the GPU can use
    // while macOS and other apps stay responsive.
    let vram_free_mb = total_mb.map(|mb| mb / 2);
    let brand = std::process::Command::new("sysctl")
        .args(["-n", "machdep.cpu.brand_string"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty());
    let fingerprint = match (brand, total_mb) {
        (Some(brand), Some(mb)) => format!("{brand}|{mb}MB-unified"),
        _ => host_fingerprint(OnnxExecutionProvider::CoreMl),
    };
    GpuInfo {
        vram_free_mb,
        fingerprint,
    }
}

fn detect_nvidia_gpu_info() -> GpuInfo {
    // Single nvidia-smi call that returns free VRAM, device name, total VRAM, and driver.
    let output = std::process::Command::new("nvidia-smi")
        .args([
            "--query-gpu=memory.free,name,memory.total,driver_version",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .ok();
    let Some(output) = output.filter(|o| o.status.success()) else {
        return GpuInfo {
            vram_free_mb: None,
            fingerprint: host_fingerprint(OnnxExecutionProvider::Cuda),
        };
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let first_line = stdout.lines().find_map(|line| {
        let trimmed = line.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    });
    let Some(line) = first_line else {
        return GpuInfo {
            vram_free_mb: None,
            fingerprint: host_fingerprint(OnnxExecutionProvider::Cuda),
        };
    };
    // CSV columns: memory.free, name, memory.total, driver_version
    let parts: Vec<&str> = line.split(',').map(str::trim).collect();
    let vram_free_mb = parts.first().and_then(|s| s.parse::<u64>().ok());
    // Fingerprint from name, total VRAM, driver (columns 1-3).
    let fingerprint = if parts.len() >= 4 {
        format!("{}|{}|{}", parts[1], parts[2], parts[3])
    } else {
        line.replace(", ", "|").replace(',', "|")
    };
    GpuInfo {
        vram_free_mb,
        fingerprint,
    }
}

fn detect_rocm_gpu_info() -> GpuInfo {
    // rocm-smi: get VRAM info and product name in one call.
    let output = std::process::Command::new("rocm-smi")
        .args(["--showproductname", "--showmeminfo", "vram", "--csv"])
        .output()
        .ok();
    let Some(output) = output.filter(|o| o.status.success()) else {
        return GpuInfo {
            vram_free_mb: None,
            fingerprint: host_fingerprint(OnnxExecutionProvider::Rocm),
        };
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_rocm_gpu_csv(&stdout)
}

fn parse_rocm_gpu_csv(stdout: &str) -> GpuInfo {
    let mut vram_free_mb = None;
    let mut fingerprint_fields: Vec<&str> = Vec::new();
    let mut free_column = None;
    let mut total_column = None;
    let mut used_column = None;
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let parts: Vec<&str> = trimmed.split(',').map(str::trim).collect();
        let is_data_row = parts.first().is_some_and(|first| {
            let first = first.to_ascii_lowercase();
            first.starts_with("gpu[") || first.starts_with("card")
        });
        if !is_data_row {
            // Header row: remember which columns hold the memory figures.
            for (index, part) in parts.iter().enumerate() {
                let label = part.to_ascii_lowercase();
                if label.contains("free memory") {
                    free_column.get_or_insert(index);
                } else if label.contains("used memory") {
                    used_column.get_or_insert(index);
                } else if label.contains("total memory") {
                    total_column.get_or_insert(index);
                }
            }
            continue;
        }
        if vram_free_mb.is_none() {
            vram_free_mb = rocm_free_memory_mb(&parts, free_column, total_column, used_column);
        }
        if fingerprint_fields.is_empty() {
            // Keep only stable fields. VRAM totals and live usage numbers
            // change between runs, and a fingerprint that drifts defeats the
            // batch-scaler profile keying it feeds.
            fingerprint_fields = parts
                .iter()
                .copied()
                .filter(|part| is_stable_fingerprint_field(part))
                .collect();
        }
    }
    GpuInfo {
        vram_free_mb,
        fingerprint: if fingerprint_fields.is_empty() {
            host_fingerprint(OnnxExecutionProvider::Rocm)
        } else {
            fingerprint_fields.join("|")
        },
    }
}

fn is_stable_fingerprint_field(part: &str) -> bool {
    let lower = part.to_ascii_lowercase();
    !part.is_empty()
        && part.parse::<u64>().is_err()
        && !lower.contains("memory")
        && !lower.contains("vram")
}

/// Free VRAM in MB from one `rocm-smi` data row. Some versions repeat the
/// labels inside each row, others put them only in the header, and some
/// report no free column at all; derive free = total - used then.
fn rocm_free_memory_mb(
    parts: &[&str],
    free_column: Option<usize>,
    total_column: Option<usize>,
    used_column: Option<usize>,
) -> Option<u64> {
    let row_label_value = |needle: &str| {
        parts
            .iter()
            .position(|part| part.to_ascii_lowercase().contains(needle))
            .and_then(|label_index| parts.get(label_index + 1))
            .and_then(|value| value.parse::<u64>().ok())
    };
    let column_value = |column: Option<usize>| {
        column
            .and_then(|index| parts.get(index))
            .and_then(|value| value.parse::<u64>().ok())
    };

    let free_bytes = row_label_value("free memory")
        .or_else(|| column_value(free_column))
        .or_else(|| {
            // u64 parsing rejects negatives; saturating_sub keeps a used >
            // total reading from wrapping.
            let total = row_label_value("total memory").or_else(|| column_value(total_column))?;
            let used = row_label_value("used memory").or_else(|| column_value(used_column))?;
            Some(total.saturating_sub(used))
        })?;
    Some(free_bytes / (1024 * 1024))
}

fn host_fingerprint(ep: OnnxExecutionProvider) -> String {
    let host = std::env::var("HOSTNAME")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            std::process::Command::new("hostname")
                .output()
                .ok()
                .filter(|o| o.status.success())
                .and_then(|o| {
                    let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
                    (!s.is_empty()).then_some(s)
                })
        })
        .unwrap_or_else(|| "unknown-host".to_string());
    format!(
        "{host}|os={}|arch={}|backend={ep}",
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rocm_csv_parses_gpu_rows_and_free_memory_column() {
        let csv = "GPU, vram total used memory (bytes), vram total free memory (bytes)\n\
GPU[0], vram total used memory (bytes), 4294967296, vram total free memory (bytes), 12884901888\n\
GPU[1], vram total used memory (bytes), 1073741824, vram total free memory (bytes), 3221225472\n";

        let info = parse_rocm_gpu_csv(csv);

        assert_eq!(info.vram_free_mb, Some(12_288));
        assert!(info.fingerprint.contains("GPU[0]"));
    }

    #[test]
    fn rocm_csv_uses_free_memory_header_for_numeric_rows() {
        let csv = "GPU, vram total used memory (bytes), vram total free memory (bytes)\n\
GPU[0], 4294967296, 12884901888\n\
GPU[1], 1073741824, 3221225472\n";

        let info = parse_rocm_gpu_csv(csv);

        assert_eq!(info.vram_free_mb, Some(12_288));
        assert!(info.fingerprint.contains("GPU[0]"));
    }

    #[test]
    fn rocm_csv_derives_free_from_total_minus_used_for_card_rows() {
        // Newer rocm-smi CSV layout: card-prefixed rows, no free column, and
        // product-name columns from --showproductname in the same table.
        let csv = "device, VRAM Total Memory (B), VRAM Total Used Memory (B), Card series\n\
card0, 17179869184, 4294967296, AMD Radeon RX 7900 XTX\n";

        let info = parse_rocm_gpu_csv(csv);

        assert_eq!(info.vram_free_mb, Some(12_288));
        assert!(info.fingerprint.contains("card0"));
        assert!(info.fingerprint.contains("AMD Radeon RX 7900 XTX"));
    }

    #[test]
    fn rocm_fingerprint_ignores_live_memory_values() {
        let header = "device, VRAM Total Memory (B), VRAM Total Used Memory (B), Card series\n";
        let idle = format!("{header}card0, 17179869184, 1073741824, AMD Radeon\n");
        let busy = format!("{header}card0, 17179869184, 16106127360, AMD Radeon\n");

        assert_eq!(
            parse_rocm_gpu_csv(&idle).fingerprint,
            parse_rocm_gpu_csv(&busy).fingerprint,
            "live VRAM usage must not leak into the device fingerprint"
        );
        assert_eq!(parse_rocm_gpu_csv(&idle).vram_free_mb, Some(15_360));
        assert_eq!(parse_rocm_gpu_csv(&busy).vram_free_mb, Some(1_024));
    }

    #[test]
    fn rocm_csv_saturates_when_used_exceeds_total() {
        let csv = "device, VRAM Total Memory (B), VRAM Total Used Memory (B)\n\
card0, 1073741824, 4294967296\n";

        assert_eq!(parse_rocm_gpu_csv(csv).vram_free_mb, Some(0));
    }

    #[test]
    fn rocm_csv_without_memory_data_reports_no_vram() {
        let csv = "device, Card series\ncard0, AMD Radeon\n";

        assert_eq!(parse_rocm_gpu_csv(csv).vram_free_mb, None);
    }
}

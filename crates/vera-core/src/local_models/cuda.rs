//! CUDA version detection and ORT platform selection.

use anyhow::Result;
use std::path::{Path, PathBuf};

#[cfg(target_os = "linux")]
use super::ort::command_exists;
use super::*;

/// `ORT_DYLIB_PATH` override, if set to a non-empty value.
pub(super) fn ort_dylib_path_from_env() -> Option<PathBuf> {
    std::env::var("ORT_DYLIB_PATH")
        .ok()
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
}

/// Get the platform-specific ONNX Runtime shared library filename.
pub(super) fn ort_lib_filename() -> String {
    if let Ok(path) = std::env::var("ORT_DYLIB_PATH")
        && !path.is_empty()
    {
        return path;
    }

    #[cfg(target_os = "windows")]
    {
        "onnxruntime.dll".to_string()
    }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        "libonnxruntime.so".to_string()
    }
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        "libonnxruntime.dylib".to_string()
    }
    #[cfg(not(any(
        target_os = "windows",
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios"
    )))]
    {
        "libonnxruntime.so".to_string()
    }
}

use crate::config::OnnxExecutionProvider;

pub(super) fn parse_cuda_major_version(value: &str) -> Option<u32> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }

    let normalized = trimmed.trim_matches(|ch: char| ch == '"' || ch.is_whitespace());
    let last_segment = normalized
        .rsplit(['\\', '/'])
        .find(|segment| !segment.is_empty())
        .unwrap_or(normalized);
    let version_segment = last_segment
        .strip_prefix('v')
        .or_else(|| last_segment.strip_prefix("cuda-"))
        .unwrap_or(last_segment);
    [version_segment, last_segment, normalized]
        .into_iter()
        .find_map(parse_cuda_major_version_tokens)
}

pub(super) fn parse_cuda_major_version_tokens(value: &str) -> Option<u32> {
    value
        .split_whitespace()
        .find_map(parse_cuda_major_version_token)
}

pub(super) fn parse_cuda_major_version_token(value: &str) -> Option<u32> {
    let token = value.trim_matches(|ch: char| ch == '"' || ch == ',' || ch == ':' || ch == '=');
    let version_token = token
        .strip_prefix('v')
        .or_else(|| token.strip_prefix("cuda-"))
        .unwrap_or(token);
    version_token
        .split(['.', '_', '-'])
        .next()
        .and_then(|major| major.parse::<u32>().ok())
}

pub(super) fn detect_cuda_major_from_cuda_path_value(value: &str) -> Option<u32> {
    parse_cuda_major_version(value).or_else(|| {
        let cuda_root = Path::new(value);
        detect_cuda_major_from_cuda_version_file(&cuda_root.join("version.json"))
            .or_else(|| detect_cuda_major_from_cuda_version_file(&cuda_root.join("version.txt")))
    })
}

pub(super) fn detect_cuda_major_from_cuda_version_file(path: &Path) -> Option<u32> {
    let contents = std::fs::read_to_string(path).ok()?;
    parse_cuda_major_from_cuda_version_metadata(&contents)
}

pub(super) fn parse_cuda_major_from_cuda_version_metadata(value: &str) -> Option<u32> {
    parse_cuda_major_from_cuda_version_json(value)
        .or_else(|| value.lines().find_map(parse_cuda_major_version))
}

pub(super) fn parse_cuda_major_from_cuda_version_json(value: &str) -> Option<u32> {
    fn find_cuda_version(value: &serde_json::Value) -> Option<u32> {
        match value {
            serde_json::Value::Object(map) => map
                .get("cuda")
                .and_then(find_cuda_version)
                .or_else(|| {
                    map.get("version")
                        .and_then(|version| version.as_str())
                        .and_then(parse_cuda_major_version)
                })
                .or_else(|| map.values().find_map(find_cuda_version)),
            serde_json::Value::Array(values) => values.iter().find_map(find_cuda_version),
            _ => None,
        }
    }

    serde_json::from_str::<serde_json::Value>(value)
        .ok()
        .and_then(|json| find_cuda_version(&json))
}

pub(super) fn detect_cuda_major_from_cuda_path_env_vars<T, U>(
    vars: impl IntoIterator<Item = (T, U)>,
) -> Option<u32>
where
    T: AsRef<str>,
    U: AsRef<str>,
{
    vars.into_iter().find_map(|(key, value)| {
        key.as_ref().strip_prefix("CUDA_PATH_V").and_then(|suffix| {
            parse_cuda_major_version(suffix)
                .or_else(|| detect_cuda_major_from_cuda_path_value(value.as_ref()))
        })
    })
}

pub(super) fn effective_cuda_major(detected_cuda_major: Option<u32>) -> u32 {
    detected_cuda_major.unwrap_or(DEFAULT_CUDA_MAJOR)
}

pub(super) fn uses_cuda13_ort(detected_cuda_major: Option<u32>) -> bool {
    effective_cuda_major(detected_cuda_major) >= CUDA_13_ORT_MIN_MAJOR
}

pub(super) fn cuda_ort_cache_dir_name(detected_cuda_major: Option<u32>) -> &'static str {
    if uses_cuda13_ort(detected_cuda_major) {
        "cuda13"
    } else {
        "cuda"
    }
}

#[cfg(any(target_os = "linux", test))]
pub(super) fn parse_cuda_major_from_runtime_library_entry(value: &str) -> Option<u32> {
    CUDA_RUNTIME_LIBRARY_PREFIXES
        .iter()
        .filter_map(|prefix| {
            value.find(prefix).and_then(|start| {
                let digits: String = value[start + prefix.len()..]
                    .chars()
                    .take_while(|ch| ch.is_ascii_digit())
                    .collect();
                (!digits.is_empty())
                    .then(|| digits.parse::<u32>().ok())
                    .flatten()
            })
        })
        .max()
}

#[cfg(test)]
pub(super) fn detect_cuda_major_from_library_entries<T>(
    entries: impl IntoIterator<Item = T>,
) -> Option<u32>
where
    T: AsRef<str>,
{
    entries
        .into_iter()
        .filter_map(|entry| parse_cuda_major_from_runtime_library_entry(entry.as_ref()))
        .max()
}

#[cfg(target_os = "linux")]
pub(super) fn detect_cuda_major_from_library_dirs<T>(
    dirs: impl IntoIterator<Item = T>,
) -> Option<u32>
where
    T: AsRef<Path>,
{
    dirs.into_iter()
        .filter_map(|dir| std::fs::read_dir(dir.as_ref()).ok())
        .flat_map(|entries| entries.filter_map(std::result::Result::ok))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter_map(|name| parse_cuda_major_from_runtime_library_entry(&name))
        .max()
}

#[cfg(target_os = "linux")]
pub(super) fn detect_cuda_major_from_library_dir_groups(groups: &[Vec<PathBuf>]) -> Option<u32> {
    groups
        .iter()
        .find_map(|dirs| detect_cuda_major_from_library_dirs(dirs.iter()))
}

#[cfg(target_os = "linux")]
pub(super) fn push_unique_library_dir(dirs: &mut Vec<PathBuf>, dir: PathBuf) {
    if !dirs.iter().any(|existing| existing == &dir) {
        dirs.push(dir);
    }
}

#[cfg(target_os = "linux")]
pub(super) fn cuda_library_dirs_from_cuda_path() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(cuda_path) = std::env::var("CUDA_PATH") {
        let base = PathBuf::from(cuda_path);
        push_unique_library_dir(&mut dirs, base.join("lib64"));
        push_unique_library_dir(&mut dirs, base.join("targets/x86_64-linux/lib"));
    }
    dirs
}

#[cfg(target_os = "linux")]
pub(super) fn cuda_library_dirs_from_ld_library_path() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(paths) = std::env::var_os("LD_LIBRARY_PATH") {
        for path in std::env::split_paths(&paths) {
            push_unique_library_dir(&mut dirs, path);
        }
    }
    dirs
}

#[cfg(target_os = "linux")]
pub(super) fn default_cuda_library_dirs() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/opt/cuda/lib64"),
        PathBuf::from("/opt/cuda/targets/x86_64-linux/lib"),
        PathBuf::from("/usr/local/cuda/lib64"),
        PathBuf::from("/usr/local/cuda/targets/x86_64-linux/lib"),
        PathBuf::from("/usr/lib64"),
        PathBuf::from("/usr/lib"),
        PathBuf::from("/usr/lib/x86_64-linux-gnu"),
    ]
}

pub(super) fn detect_cuda_major_from_cuda_path() -> Option<u32> {
    std::env::var("CUDA_PATH")
        .ok()
        .and_then(|value| detect_cuda_major_from_cuda_path_value(&value))
        .or_else(|| detect_cuda_major_from_cuda_path_env_vars(std::env::vars()))
}

pub(super) fn detect_cuda_major_from_nvcc() -> Option<u32> {
    let output = std::process::Command::new("nvcc")
        .arg("--version")
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let release = stdout.split("release ").nth(1)?;
    release
        .split([',', '\n', '\r'])
        .next()
        .and_then(parse_cuda_major_version)
}

pub(super) fn detect_cuda_major_from_nvidia_smi() -> Option<u32> {
    let output = std::process::Command::new("nvidia-smi").output().ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let rest = stdout.split("CUDA Version:").nth(1)?;
    rest.split_whitespace()
        .next()
        .and_then(parse_cuda_major_version)
}

#[cfg(target_os = "linux")]
pub(super) fn detect_cuda_major_from_runtime_libraries() -> Option<u32> {
    let search_groups = [
        cuda_library_dirs_from_cuda_path(),
        cuda_library_dirs_from_ld_library_path(),
    ];
    detect_cuda_major_from_library_dir_groups(&search_groups)
        .or_else(detect_cuda_major_from_ldconfig)
        .or_else(|| detect_cuda_major_from_library_dirs(default_cuda_library_dirs()))
}

#[cfg(not(target_os = "linux"))]
pub(super) fn detect_cuda_major_from_runtime_libraries() -> Option<u32> {
    None
}

#[cfg(target_os = "linux")]
pub(super) fn detect_cuda_major_from_ldconfig() -> Option<u32> {
    if !command_exists("ldconfig", &["-p"]) {
        return None;
    }
    let output = std::process::Command::new("ldconfig")
        .arg("-p")
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    detect_cuda_major_from_ldconfig_entries(stdout.lines())
}

#[cfg(target_os = "linux")]
pub(super) fn detect_cuda_major_from_ldconfig_entries<T>(
    entries: impl IntoIterator<Item = T>,
) -> Option<u32>
where
    T: AsRef<str>,
{
    entries
        .into_iter()
        .filter_map(|entry| {
            let entry = entry.as_ref();
            ldconfig_entry_matches_host_arch(entry)
                .then(|| parse_cuda_major_from_runtime_library_entry(entry))
                .flatten()
        })
        .max()
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub(super) fn ldconfig_entry_matches_host_arch(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    if [
        "x86-64",
        "x86_64",
        "/lib64/",
        "/usr/lib64/",
        "/x86_64-linux-gnu/",
        "/targets/x86_64-linux/",
    ]
    .iter()
    .any(|marker| value.contains(marker))
    {
        return true;
    }

    ![
        "aarch64",
        "arm64",
        "armhf",
        "armv7",
        "armv8",
        "i386",
        "i486",
        "i586",
        "i686",
        "ppc64",
        "s390x",
        "riscv",
        "/lib32/",
        "/usr/lib32/",
        "/aarch64-linux-gnu/",
        "/arm-linux-gnueabihf/",
        "/i386-linux-gnu/",
        "/i686-linux-gnu/",
        "/ppc64le-linux-gnu/",
        "/s390x-linux-gnu/",
        "/riscv64-linux-gnu/",
    ]
    .iter()
    .any(|marker| value.contains(marker))
}

#[cfg(all(target_os = "linux", not(target_arch = "x86_64")))]
pub(super) fn ldconfig_entry_matches_host_arch(_: &str) -> bool {
    true
}

/// Detect the CUDA toolkit major version. Prefer the installed toolkit over
/// the driver's maximum supported version so Vera picks the matching ORT build.
pub(super) fn detect_cuda_major_version() -> Option<u32> {
    detect_cuda_major_from_cuda_path()
        .or_else(detect_cuda_major_from_nvcc)
        .or_else(detect_cuda_major_from_runtime_libraries)
        .or_else(detect_cuda_major_from_nvidia_smi)
}

pub(super) fn detected_cuda_major_for_ep(ep: OnnxExecutionProvider) -> Option<u32> {
    matches!(ep, OnnxExecutionProvider::Cuda)
        .then(detect_cuda_major_version)
        .flatten()
}

/// Platform-specific ORT archive info: (archive_ext, archive_name, primary_lib_path_inside_archive, local_lib_name, version).
pub(super) fn ort_platform_info_with_cuda_major(
    ep: OnnxExecutionProvider,
    detected_cuda_major: Option<u32>,
) -> Result<(&'static str, String, String, &'static str, &'static str)> {
    let gpu_suffix = match ep {
        OnnxExecutionProvider::Cpu => "",
        OnnxExecutionProvider::Cuda => {
            let cuda_major = effective_cuda_major(detected_cuda_major);
            if uses_cuda13_ort(detected_cuda_major) {
                tracing::info!("detected CUDA {cuda_major}, using CUDA 13 ORT build");
                "-gpu_cuda13"
            } else {
                tracing::info!("detected CUDA {cuda_major}, using CUDA 12 ORT build");
                "-gpu"
            }
        }
        OnnxExecutionProvider::Rocm => "-rocm",
        OnnxExecutionProvider::DirectMl => "-directml",
        OnnxExecutionProvider::CoreMl => "",
        OnnxExecutionProvider::OpenVino => "-openvino",
    };

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        if matches!(ep, OnnxExecutionProvider::DirectMl) {
            anyhow::bail!("DirectML is only supported on Windows");
        }
        if matches!(
            ep,
            OnnxExecutionProvider::OpenVino | OnnxExecutionProvider::Rocm
        ) {
            // These EPs are installed via pip wheels, not GitHub release archives.
            // Return a dummy value; `ensure_ort_library_for_ep` handles them separately.
            let base = format!("onnxruntime-linux-x64{gpu_suffix}-{ORT_VERSION}");
            return Ok((
                "tgz",
                base.clone(),
                format!("{base}/lib/libonnxruntime.so.{ORT_VERSION}"),
                "libonnxruntime.so",
                ORT_VERSION,
            ));
        }
        // The CUDA 13 archive is named with `_cuda13` in the filename, but the
        // internal directory inside the tgz always uses plain `-gpu` (no `_cuda13`).
        let archive_name = format!("onnxruntime-linux-x64{gpu_suffix}-{ORT_VERSION}");
        let internal_gpu_suffix = if matches!(ep, OnnxExecutionProvider::Cuda) {
            "-gpu"
        } else {
            gpu_suffix
        };
        let internal_base = format!("onnxruntime-linux-x64{internal_gpu_suffix}-{ORT_VERSION}");
        Ok((
            "tgz",
            archive_name,
            format!("{internal_base}/lib/libonnxruntime.so.{ORT_VERSION}"),
            "libonnxruntime.so",
            ORT_VERSION,
        ))
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        if !matches!(ep, OnnxExecutionProvider::Cpu) {
            anyhow::bail!("Only CPU execution provider is supported on Linux aarch64");
        }
        let base = format!("onnxruntime-linux-aarch64-{ORT_VERSION}");
        Ok((
            "tgz",
            base.clone(),
            format!("{base}/lib/libonnxruntime.so.{ORT_VERSION}"),
            "libonnxruntime.so",
            ORT_VERSION,
        ))
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        if !matches!(
            ep,
            OnnxExecutionProvider::Cpu | OnnxExecutionProvider::CoreMl
        ) {
            anyhow::bail!("Only CPU and CoreML execution providers are supported on macOS ARM");
        }
        let base = format!("onnxruntime-osx-arm64-{ORT_VERSION}");
        Ok((
            "tgz",
            base.clone(),
            format!("{base}/lib/libonnxruntime.{ORT_VERSION}.dylib"),
            "libonnxruntime.dylib",
            ORT_VERSION,
        ))
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        if !matches!(ep, OnnxExecutionProvider::Cpu) {
            anyhow::bail!("Only CPU execution provider is supported on macOS x86_64");
        }
        let ver = ORT_VERSION_MACOS_X86;
        let base = format!("onnxruntime-osx-x86_64-{ver}");
        Ok((
            "tgz",
            base.clone(),
            format!("{base}/lib/libonnxruntime.{ver}.dylib"),
            "libonnxruntime.dylib",
            ORT_VERSION_MACOS_X86,
        ))
    }
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        if matches!(
            ep,
            OnnxExecutionProvider::Rocm | OnnxExecutionProvider::OpenVino
        ) {
            anyhow::bail!("ROCm and OpenVINO are only supported on Linux x86_64");
        }
        // The CUDA 13 archive is named with `_cuda13` in the filename, but the
        // internal directory inside the zip always uses plain `-gpu` (no `_cuda13`).
        let archive_name = format!("onnxruntime-win-x64{gpu_suffix}-{ORT_VERSION}");
        // Internal paths inside the zip always use the plain gpu suffix (no _cuda13).
        let internal_gpu_suffix = if matches!(ep, OnnxExecutionProvider::Cuda) {
            "-gpu"
        } else {
            gpu_suffix
        };
        let internal_base = format!("onnxruntime-win-x64{internal_gpu_suffix}-{ORT_VERSION}");
        let entry = format!("{internal_base}/lib/onnxruntime.dll");
        Ok(("zip", archive_name, entry, "onnxruntime.dll", ORT_VERSION))
    }
    #[cfg(not(any(
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64"),
        all(target_os = "windows", target_arch = "x86_64"),
    )))]
    {
        let _ = (ep, gpu_suffix);
        anyhow::bail!(
            "Unsupported platform for automatic ONNX Runtime download. \
             Install ONNX Runtime manually and set ORT_DYLIB_PATH."
        )
    }
}

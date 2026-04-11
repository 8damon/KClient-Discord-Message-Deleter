use std::{
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};

const APP_DIR_NAME: &str = "kcordclient";
const APP_ROOT_OVERRIDE: &str = "KCLIENT_APP_ROOT";

pub fn app_root_dir() -> PathBuf {
    if let Some(dir) = env::var_os(APP_ROOT_OVERRIDE) {
        return PathBuf::from(dir);
    }
    if let Some(dir) = preferred_data_dir() {
        return dir.join(APP_DIR_NAME);
    }
    Path::new(APP_DIR_NAME).to_path_buf()
}

pub fn data_dir() -> PathBuf {
    app_root_dir().join("data")
}

pub fn logs_dir() -> PathBuf {
    app_root_dir().join("logs")
}

pub fn ensure_app_dirs() -> Result<()> {
    let root = app_root_dir();
    fs::create_dir_all(&root)
        .with_context(|| format!("failed to create app dir {}", root.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).ok();
    }
    Ok(())
}

pub fn uninstall_paths() -> Vec<PathBuf> {
    let mut candidates = vec![app_root_dir()];

    if let Ok(current_dir) = env::current_dir() {
        candidates.push(current_dir.join(APP_DIR_NAME));
    }

    if let Ok(exe_path) = env::current_exe() {
        if let Some(parent) = exe_path.parent() {
            candidates.push(parent.join(APP_DIR_NAME));
            if let Some(grandparent) = parent.parent() {
                candidates.push(grandparent.join(APP_DIR_NAME));
                if let Some(root) = grandparent.parent() {
                    candidates.push(root.join(APP_DIR_NAME));
                }
            }
        }
    }

    dedupe_paths(candidates)
}

fn preferred_data_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        env::var_os("APPDATA").map(PathBuf::from)
    }
    #[cfg(target_os = "linux")]
    {
        env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| {
            env::var_os("HOME").map(|home| PathBuf::from(home).join(".local").join("share"))
        })
    }
    #[cfg(target_os = "macos")]
    {
        env::var_os("HOME").map(|home| {
            PathBuf::from(home)
                .join("Library")
                .join("Application Support")
        })
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

fn dedupe_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut unique = Vec::new();

    for path in paths {
        if !unique.iter().any(|existing| existing == &path) {
            unique.push(path);
        }
    }

    unique
}

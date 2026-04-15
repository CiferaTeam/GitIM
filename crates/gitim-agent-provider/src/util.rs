use std::path::{Path, PathBuf};

/// Find an executable by name, checking absolute paths and PATH.
pub fn which(name: &str) -> Result<PathBuf, ()> {
    let path = Path::new(name);
    if path.is_absolute() && path.exists() {
        return Ok(path.to_path_buf());
    }
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in path_var.split(':') {
            let full = Path::new(dir).join(name);
            if full.exists() {
                return Ok(full);
            }
        }
    }
    Err(())
}

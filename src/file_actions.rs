// SPDX-License-Identifier: GPL-3.0-or-later
//! Explicit, reviewed Trash actions from the folder browser. Never AI actions.
use crate::{cache::PathIdentity, whitelist::Whitelist};
use std::os::unix::fs::MetadataExt;
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone)]
pub(crate) struct TrashPlan {
    pub path: PathBuf,
    pub size_kb: u64,
    home: PathBuf,
    identity: PathIdentity,
}

impl TrashPlan {
    pub fn prepare(path: &Path, home: &Path, size_kb: u64) -> Result<Self, String> {
        validate(path, home)?;
        let identity = PathIdentity::capture(path)
            .ok_or("Could not verify this item's identity. Measure it again.")?;
        Ok(Self {
            path: path.into(),
            home: home.into(),
            size_kb,
            identity,
        })
    }

    fn revalidate(&self) -> Result<(), String> {
        validate(&self.path, &self.home)?;
        if PathIdentity::capture(&self.path) != Some(self.identity) {
            return Err(
                "The item or its parent changed since you selected it. Select it again.".into(),
            );
        }
        Ok(())
    }

    pub fn execute(&self) -> Result<(), String> {
        self.revalidate()?;
        #[cfg(target_os = "macos")]
        {
            use trash::macos::{DeleteMethod, TrashContextExtMacos};
            let mut context = trash::TrashContext::default();
            // Native Trash, without Finder automation or a permanent-delete fallback.
            context.set_delete_method(DeleteMethod::NsFileManager);
            context.delete(&self.path).map_err(|e| e.to_string())
        }
        #[cfg(not(target_os = "macos"))]
        Err("Moving to Trash is supported only on macOS.".into())
    }
}

fn validate(path: &Path, home: &Path) -> Result<(), String> {
    let home = home.canonicalize().map_err(|e| e.to_string())?;
    // Do not follow any symlink on the route to a manually selected item.
    if !path.is_absolute() || path.to_str().is_none() {
        return Err("This path cannot be moved to Trash here. Review it in Finder.".into());
    }
    let resolved = path.canonicalize().map_err(|e| e.to_string())?;
    if resolved != path || !resolved.starts_with(&home) || resolved == home {
        return Err("Trash is limited to items inside your home, without redirected paths.".into());
    }
    let relative = resolved.strip_prefix(&home).map_err(|e| e.to_string())?;
    let first = relative
        .components()
        .next()
        .ok_or("Choose an item inside a folder.")?;
    let first = first.as_os_str().to_string_lossy().to_lowercase();
    let cache_child = relative.starts_with("Library/Caches") && relative.components().count() > 2;
    if (first == "library" && !cache_child)
        || matches!(first.as_str(), ".trash" | ".ssh" | ".gnupg")
        || (relative.components().count() == 1
            && matches!(
                first.as_str(),
                "desktop"
                    | "documents"
                    | "downloads"
                    | "movies"
                    | "music"
                    | "pictures"
                    | "public"
                    | "applications"
            ))
    {
        return Err(
            "This location is protected. Browse its contents or use an app-specific cleanup rule."
                .into(),
        );
    }
    if std::env::current_dir().is_ok_and(|cwd| cwd.starts_with(&resolved)) {
        return Err(
            "This contains Diskray's working directory. Choose a child item instead.".into(),
        );
    }
    if Whitelist::load(&home).keeps(path) {
        return Err("This item or something inside it is protected by your whitelist.".into());
    }
    let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    let owner = fs::metadata(&home).map_err(|e| e.to_string())?;
    if !(metadata.is_file() || metadata.is_dir())
        || metadata.uid() != owner.uid()
        || metadata.dev() != owner.dev()
    {
        return Err("Choose an owned file or folder on your home volume.".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "moves a generated fixture to native macOS Trash and restores it"]
    fn native_trash_round_trip() {
        let home = PathBuf::from(std::env::var_os("HOME").unwrap())
            .canonicalize()
            .unwrap();
        let trash = home.join(".Trash");
        // Check access before moving anything, so the fixture can be recovered.
        fs::read_dir(&trash).unwrap();
        let fixture = tempfile::Builder::new()
            .prefix(".diskray-trash-test-")
            .tempdir_in(&home)
            .unwrap();
        let name = format!(
            "{}.txt",
            fixture.path().file_name().unwrap().to_string_lossy()
        );
        let source = fixture.path().join(&name);
        let destination = trash.join(&name);
        assert!(!destination.exists());
        fs::write(&source, "Diskray disposable native Trash test").unwrap();
        let plan = TrashPlan::prepare(&source, &home, 4).unwrap();
        plan.execute().unwrap();
        assert!(!source.exists());
        assert_eq!(
            fs::read_to_string(&destination).unwrap(),
            "Diskray disposable native Trash test"
        );
        fs::rename(destination, &source).unwrap();
        assert!(source.exists());
        // TempDir removes only this restored test fixture.
    }
    #[test]
    fn trash_revalidates_identity_and_new_whitelist_without_deleting() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().canonicalize().unwrap();
        let target = home.join("project");
        fs::create_dir(&target).unwrap();
        let plan = TrashPlan::prepare(&target, &home, 8).unwrap();
        fs::rename(&target, home.join("original")).unwrap();
        fs::create_dir(&target).unwrap();
        assert!(plan.revalidate().unwrap_err().contains("changed"));
        let plan = TrashPlan::prepare(&target, &home, 8).unwrap();
        let whitelist = crate::whitelist::whitelist_path(&home);
        fs::create_dir_all(whitelist.parent().unwrap()).unwrap();
        fs::write(whitelist, "project/important\n").unwrap();
        assert!(plan.revalidate().unwrap_err().contains("whitelist"));
        assert!(target.exists());
    }
    #[test]
    fn trash_protects_anchors_and_symlinks_but_allows_selected_children() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().canonicalize().unwrap();
        for path in [
            "Library",
            "Library/Caches",
            "Library/Application Support/app",
            ".Trash/stuff",
            ".ssh",
            "Documents",
        ] {
            let path = home.join(path);
            fs::create_dir_all(&path).unwrap();
            assert!(TrashPlan::prepare(&path, &home, 0).is_err());
        }
        assert!(TrashPlan::prepare(&home, &home, 0).is_err());
        let target = home.join("Documents/old-project");
        fs::create_dir(&target).unwrap();
        assert!(TrashPlan::prepare(&target, &home, 0).is_ok());
        let cache = home.join("Library/Caches/selected-cache");
        fs::create_dir(&cache).unwrap();
        assert!(TrashPlan::prepare(&cache, &home, 0).is_ok());
        std::os::unix::fs::symlink(home.join("Documents"), home.join("link")).unwrap();
        assert!(TrashPlan::prepare(&home.join("link/old-project"), &home, 0).is_err());
    }
}

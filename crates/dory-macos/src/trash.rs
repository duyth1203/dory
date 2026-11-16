//! `MacTrash`: the Finder's Trash, through `NSFileManager trashItemAtURL:resultingItemURL:error:`.
//!
//! The receipt keeps each original path and the path the system gave the item in the Trash, which
//! may be renamed to avoid a clash. `put_back` renames them straight back, so an item returns
//! even though it carries no Finder "Put Back" metadata. objc2 declares these `NSFileManager`
//! methods safe, so this module has no `unsafe`.

use std::path::{Path, PathBuf};

use dory_core::ports::{Moved, Trash, TrashError, TrashReceipt};
use objc2_foundation::{NSFileManager, NSString, NSURL};

#[derive(Default)]
pub struct MacTrash;

impl MacTrash {
    pub fn new() -> Self {
        Self
    }
}

fn url(path: &Path) -> Result<objc2::rc::Retained<NSURL>, TrashError> {
    let text = path
        .to_str()
        .ok_or_else(|| TrashError::Platform(format!("{} is not UTF-8", path.display())))?;
    Ok(NSURL::fileURLWithPath(&NSString::from_str(text)))
}

fn io(path: &Path, source: std::io::Error) -> TrashError {
    TrashError::Io {
        path: path.to_owned(),
        source,
    }
}

impl Trash for MacTrash {
    fn move_to_trash(&self, paths: &[PathBuf]) -> Result<TrashReceipt, TrashError> {
        let manager = NSFileManager::defaultManager();
        let mut moved = Vec::new();
        for original in paths {
            let outcome = url(original).and_then(|from| {
                let mut resulting = None;
                manager
                    .trashItemAtURL_resultingItemURL_error(&from, Some(&mut resulting))
                    .map_err(|e| TrashError::Platform(e.localizedDescription().to_string()))?;
                resulting
                    .and_then(|to| to.path())
                    .map(|to| PathBuf::from(to.to_string()))
                    .ok_or_else(|| {
                        TrashError::Platform(format!(
                            "the system did not say where it put {}",
                            original.display()
                        ))
                    })
            });
            match outcome {
                Ok(trashed) => moved.push(Moved {
                    original: original.clone(),
                    trashed,
                }),
                Err(error) => {
                    let _ = self.put_back(&TrashReceipt::new(moved));
                    return Err(error);
                }
            }
        }
        Ok(TrashReceipt::new(moved))
    }

    fn put_back(&self, receipt: &TrashReceipt) -> Result<(), TrashError> {
        for item in receipt.moved() {
            if std::fs::symlink_metadata(&item.trashed).is_err() {
                return Err(TrashError::Missing(item.trashed.clone()));
            }
            if std::fs::symlink_metadata(&item.original).is_ok() {
                return Err(TrashError::Occupied(item.original.clone()));
            }
        }
        let mut back: Vec<&Moved> = Vec::new();
        for item in receipt.moved() {
            let renamed = match item.original.parent() {
                Some(parent) => std::fs::create_dir_all(parent),
                None => Ok(()),
            }
            .and_then(|()| std::fs::rename(&item.trashed, &item.original));
            if let Err(source) = renamed {
                for done in back {
                    let _ = std::fs::rename(&done.original, &done.trashed);
                }
                return Err(io(&item.original, source));
            }
            back.push(item);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dory-trash-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn an_empty_list_moves_nothing() {
        let receipt = MacTrash::new().move_to_trash(&[]).unwrap();
        assert!(receipt.is_empty());
        MacTrash::new().put_back(&receipt).unwrap();
    }

    #[test]
    fn a_missing_file_is_an_error_and_moves_nothing_else() {
        let dir = scratch("missing");
        let real = dir.join("real.txt");
        std::fs::write(&real, "x").unwrap();
        let result = MacTrash::new().move_to_trash(&[real.clone(), dir.join("absent.txt")]);
        assert!(result.is_err());
        assert!(real.exists(), "the first file went back");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn put_back_refuses_to_overwrite_and_reports_a_file_gone_from_the_trash() {
        let dir = scratch("refuse");
        let receipt = TrashReceipt::new(vec![Moved {
            original: dir.join("a"),
            trashed: dir.join("gone-from-trash"),
        }]);
        assert!(matches!(
            MacTrash::new().put_back(&receipt),
            Err(TrashError::Missing(_))
        ));
        std::fs::write(dir.join("gone-from-trash"), "t").unwrap();
        std::fs::write(dir.join("a"), "occupied").unwrap();
        assert!(matches!(
            MacTrash::new().put_back(&receipt),
            Err(TrashError::Occupied(_))
        ));
        assert_eq!(std::fs::read(dir.join("a")).unwrap(), b"occupied");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Touches the real `~/.Trash`: run with `cargo test -p dory-macos -- --ignored`.
    #[test]
    #[ignore = "puts a file in the user's real Trash"]
    fn a_file_goes_to_the_real_trash_and_comes_back_with_its_bytes() {
        let dir = scratch("real");
        let file = dir.join(format!("dory-trash-test-{}.txt", std::process::id()));
        std::fs::write(&file, "dory trash test").unwrap();

        let trash = MacTrash::new();
        let receipt = trash.move_to_trash(std::slice::from_ref(&file)).unwrap();
        let [moved] = receipt.moved() else {
            panic!("one file was moved");
        };
        assert!(!file.exists());
        assert!(moved.trashed.exists());
        assert!(moved.trashed.to_string_lossy().contains("/.Trash/"));
        println!("trashed at {}", moved.trashed.display());

        trash.put_back(&receipt).unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"dory trash test");
        assert!(!moved.trashed.exists());

        let again = trash.move_to_trash(std::slice::from_ref(&file)).unwrap();
        println!("left in the Trash: {}", again.moved()[0].trashed.display());
        std::fs::remove_dir_all(dir).unwrap();
    }
}

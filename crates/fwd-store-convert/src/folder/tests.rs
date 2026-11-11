use std::fs;
use std::path::{Path, PathBuf};

use super::{survey, FolderError, Item, Staging};
use crate::store::tests::write_old_store;

/// A data folder with one project's store and the files fwd keeps beside it.
fn data_folder(root: &Path) -> PathBuf {
    let data = root.join("data");
    fs::create_dir_all(data.join("projects/p1")).unwrap();
    fs::write(data.join("projects.toml"), "[[projects]]\n").unwrap();
    fs::write(data.join("projects/p1/checkouts.json"), "[]").unwrap();
    write_old_store(&data.join("projects/p1/project.sqlite"));
    data
}

#[test]
fn stores_are_found_where_fwd_keeps_them_and_everything_else_is_carried() {
    let root = tempfile::tempdir().unwrap();
    let data = data_folder(root.path());
    assert_eq!(
        survey(&data).unwrap(),
        vec![
            Item::Dir("projects".into()),
            Item::Dir("projects/p1".into()),
            Item::File("projects/p1/checkouts.json".into()),
            Item::Store { project: "p1".into(), path: "projects/p1/project.sqlite".into() },
            Item::File("projects.toml".into()),
        ]
    );
}

#[test]
fn a_write_ahead_log_beside_a_database_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let data = data_folder(root.path());
    fs::write(data.join("projects/p1/project.sqlite-wal"), "").unwrap();
    assert!(matches!(survey(&data), Err(FolderError::Busy(path)) if path.ends_with("project.sqlite-wal")));
}

#[test]
fn an_ankurah_store_outside_a_project_folder_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let data = data_folder(root.path());
    fs::copy(data.join("projects/p1/project.sqlite"), data.join("stray.sqlite")).unwrap();
    assert!(matches!(survey(&data), Err(FolderError::StrayStore(path)) if path.ends_with("stray.sqlite")));
}

#[test]
fn a_folder_without_an_index_is_not_a_data_folder() {
    let root = tempfile::tempdir().unwrap();
    assert!(matches!(survey(root.path()), Err(FolderError::NotDataFolder(_))));
}

#[test]
fn a_destination_that_exists_or_lies_inside_the_source_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let data = data_folder(root.path());
    assert!(matches!(Staging::create(&data, &data.join("converted")), Err(FolderError::Overlap { .. })));
    fs::create_dir(root.path().join("taken")).unwrap();
    assert!(matches!(Staging::create(&data, &root.path().join("taken")), Err(FolderError::Exists(_))));
}

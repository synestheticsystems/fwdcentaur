use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use super::Staging;

/// A folder's permissions, in octal.
fn mode(path: &Path) -> String {
    format!("{:o}", fs::metadata(path).unwrap().permissions().mode() & 0o777)
}

/// The staging folder holds every project's records while the move runs, so only its owner may
/// read it then; once it settles under the destination's name, it has the source folder's mode.
#[test]
fn the_staging_folder_is_its_owners_alone_until_it_settles_with_the_sources_mode() {
    let root = tempfile::tempdir().unwrap();
    let from = root.path().join("data");
    fs::create_dir(&from).unwrap();
    fs::set_permissions(&from, fs::Permissions::from_mode(0o750)).unwrap();

    let staging = Staging::create(&from, &root.path().join("merged")).unwrap();
    assert_eq!(mode(&staging.path), "700", "while the move runs");

    let to = staging.settle(&from).unwrap();
    assert_eq!(mode(&to), "750", "once it settles");
}

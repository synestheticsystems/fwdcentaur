use std::fs;
use std::path::Path;

use super::{convert, Args};
use crate::store::tests::write_old_store;

fn data_folder(root: &Path) -> Args {
    let from = root.join("data");
    fs::create_dir_all(from.join("projects/p1")).unwrap();
    fs::write(from.join("projects.toml"), "[[projects]]\n").unwrap();
    fs::write(from.join("daemon.log"), "a line\n").unwrap();
    write_old_store(&from.join("projects/p1/project.sqlite"));
    Args { from, to: root.join("converted") }
}

#[tokio::test]
async fn a_converted_folder_holds_every_other_file_as_it_was() {
    let root = tempfile::tempdir().unwrap();
    let args = data_folder(root.path());

    assert!(convert(&args).await.unwrap());
    for file in ["projects.toml", "daemon.log"] {
        assert_eq!(fs::read(args.to.join(file)).unwrap(), fs::read(args.from.join(file)).unwrap(), "{file}");
    }
    assert!(args.to.join("projects/p1/project.sqlite").is_file());
    assert_eq!(
        fs::read_dir(root.path()).unwrap().count(),
        2,
        "only the source and the destination: no staging folder is left"
    );
}

#[tokio::test]
async fn a_refused_store_leaves_nothing_at_the_destination() {
    let root = tempfile::tempdir().unwrap();
    let args = data_folder(root.path());
    fs::create_dir_all(args.from.join("projects/p2")).unwrap();
    write_old_store(&args.from.join("projects/p2/project.sqlite"));
    rusqlite::Connection::open(args.from.join("projects/p2/project.sqlite"))
        .unwrap()
        .execute(r#"CREATE TABLE "kept_by_hand" ("x" TEXT)"#, [])
        .unwrap();

    assert!(!convert(&args).await.unwrap());
    assert!(!args.to.exists());
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1, "the staging folder is gone too");
}

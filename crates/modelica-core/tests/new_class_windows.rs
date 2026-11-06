#![cfg(windows)]

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use modelica_core::{
    ClassKind, NewClassContext, NewClassRequest, NewClassStorageMode, apply_new_class_plan,
    plan_new_class,
};

fn test_directory(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    let manifest_directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_directory
        .parent()
        .and_then(Path::parent)
        .expect("workspace root");
    let base = std::env::var_os("MODELICA_CORE_WINDOWS_TEST_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace_root.join("target"));
    let path = base.join(format!(
        "modelica-core-windows-{label}-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir_all(&path).expect("create Windows integration-test directory");
    path
}

fn standalone_model(name: &str, directory: PathBuf) -> NewClassRequest {
    NewClassRequest {
        name: name.to_owned(),
        kind: ClassKind::Model,
        description: None,
        partial: false,
        base_class: None,
        storage: NewClassStorageMode::SingleFile {
            directory,
            within: None,
            package_order_file: None,
            package_order_before: None,
        },
    }
}

#[test]
fn windows_creates_new_model_without_overwriting_neighbors_or_leaving_temp_files() {
    let directory = test_directory("create-and-preserve");
    let existing = directory.join("IEH_CPP.mo");
    fs::write(&existing, "model IEH_CPP\nend IEH_CPP;\n").expect("write neighbor model");
    let plan = plan_new_class(
        &standalone_model("Model1", directory.clone()),
        &NewClassContext::default(),
    )
    .expect("plan standalone class");

    let created = apply_new_class_plan(&plan).expect("create Model1 on Windows filesystem");
    assert_eq!(created, directory.join("Model1.mo"));
    assert_eq!(
        fs::read_to_string(&existing).expect("read neighbor model"),
        "model IEH_CPP\nend IEH_CPP;\n"
    );
    assert_eq!(
        fs::read_to_string(&created).expect("read created model"),
        "model Model1\nend Model1;\n"
    );
    let files = fs::read_dir(&directory)
        .expect("list test directory")
        .map(|entry| entry.expect("directory entry").file_name())
        .collect::<Vec<_>>();
    assert_eq!(files.len(), 2, "no temporary file may be left behind");
    assert!(
        files
            .iter()
            .all(|name| !name.to_string_lossy().ends_with(".tmp"))
    );

    fs::remove_dir_all(directory).expect("remove test fixture");
}

#[test]
fn windows_refuses_existing_destination_and_preserves_both_files() {
    let directory = test_directory("existing-destination");
    let existing = directory.join("IEH_CPP.mo");
    let destination = directory.join("Model1.mo");
    fs::write(&existing, "model IEH_CPP\nend IEH_CPP;\n").expect("write neighbor model");
    let plan = plan_new_class(
        &standalone_model("Model1", directory.clone()),
        &NewClassContext::default(),
    )
    .expect("plan while destination is initially free");
    fs::write(
        &destination,
        "model Model1\n  Real keep = 7;\nend Model1;\n",
    )
    .expect("simulate a destination created after planning");

    let error = apply_new_class_plan(&plan).expect_err("must never overwrite existing .mo");
    assert!(error.contains("拒绝覆盖已有文件"), "{error}");
    assert_eq!(
        fs::read_to_string(&existing).expect("read neighbor model"),
        "model IEH_CPP\nend IEH_CPP;\n"
    );
    assert_eq!(
        fs::read_to_string(&destination).expect("read existing destination"),
        "model Model1\n  Real keep = 7;\nend Model1;\n"
    );
    fs::remove_dir_all(directory).expect("remove test fixture");
}

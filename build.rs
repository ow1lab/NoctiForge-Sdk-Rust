fn main() {
    let repo_dir = tempfile::tempdir().unwrap();

    git2::Repository::clone(
        "https://github.com/ow1lab/NoctiForge-proto",
        &repo_dir)
    .unwrap();

    let proto_dir = repo_dir.path().join("function/v0");

    tonic_prost_build::configure()
        .build_client(false)
        .build_server(true)
        .compile_protos(
        &[
            proto_dir.join("action.proto"),
        ],
        &[proto_dir],
    ).unwrap();
}

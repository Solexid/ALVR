use crate::{command, dependencies, version};
use alvr_filesystem::{self as afs, Layout};
use xshell::{cmd, Shell};

pub fn build_server(
    is_release: bool,
    gpl: bool,
    root: Option<String>,
    reproducible: bool,
    experiments: bool,
) {
    let sh = Shell::new().unwrap();

    let build_layout = Layout::new(&afs::server_build_dir());

    let build_type = if is_release { "release" } else { "debug" };

    let mut common_flags = vec![];
    if is_release {
        common_flags.push("--release");
    }
    if reproducible {
        common_flags.push("--locked");
    }
    let common_flags_ref = &common_flags;

    let gpl_flag = gpl.then(|| vec!["--features", "gpl"]).unwrap_or_default();

    let artifacts_dir = afs::target_dir().join(build_type);

    sh.remove_path(&afs::server_build_dir()).unwrap();
    sh.create_dir(&afs::server_build_dir()).unwrap();
    sh.create_dir(&build_layout.openvr_driver_lib_dir())
        .unwrap();
    sh.create_dir(&build_layout.executables_dir).unwrap();

    if let Some(root) = root {
        sh.set_var("ALVR_ROOT_DIR", root);
    }

    // build server
    {
        let _push_guard = sh.push_dir(afs::crate_dir("server"));
        cmd!(sh, "cargo build {common_flags_ref...} {gpl_flag...}")
            .run()
            .unwrap();

        sh.copy_file(
            artifacts_dir.join(afs::dynlib_fname("alvr_server")),
            build_layout.openvr_driver_lib(),
        )
        .unwrap();
    }

    // build launcher
    {
        let _push_guard = sh.push_dir(afs::crate_dir("launcher"));
        cmd!(sh, "cargo build {common_flags_ref...}").run().unwrap();

        sh.copy_file(
            artifacts_dir.join(afs::exec_fname("alvr_launcher")),
            build_layout.launcher_exe(),
        )
        .unwrap();
    }

    // copy dependencies
    if cfg!(windows) {
        command::copy_recursive(
            &sh,
            &afs::crate_dir("server").join("cpp/bin/windows"),
            &build_layout.openvr_driver_lib_dir(),
        )
        .unwrap();

        // copy ffmpeg binaries
        if gpl {
            let bin_dir = &build_layout.openvr_driver_lib_dir();
            sh.create_dir(&bin_dir).unwrap();
            for lib_path in sh
                .read_dir(afs::deps_dir().join("windows/ffmpeg/bin"))
                .unwrap()
                .into_iter()
                .filter(|path| path.file_name().unwrap().to_string_lossy().contains(".dll"))
            {
                sh.copy_file(lib_path.clone(), bin_dir).unwrap();
            }
        }
    } else if cfg!(target_os = "linux") {
        // build compositor wrapper
        let _push_guard = sh.push_dir(afs::crate_dir("vrcompositor_wrapper"));
        cmd!(sh, "cargo build {common_flags_ref...}").run().unwrap();
        sh.create_dir(&build_layout.vrcompositor_wrapper_dir)
            .unwrap();
        sh.copy_file(
            artifacts_dir.join("alvr_vrcompositor_wrapper"),
            build_layout.vrcompositor_wrapper(),
        )
        .unwrap();

        // build vulkan layer
        let _push_guard = sh.push_dir(afs::crate_dir("vulkan_layer"));
        cmd!(sh, "cargo build {common_flags_ref...}").run().unwrap();
        sh.create_dir(&build_layout.libraries_dir).unwrap();
        sh.copy_file(
            artifacts_dir.join(afs::dynlib_fname("alvr_vulkan_layer")),
            build_layout.vulkan_layer(),
        )
        .unwrap();

        // copy vulkan layer manifest
        sh.create_dir(&build_layout.vulkan_layer_manifest_dir)
            .unwrap();
        sh.copy_file(
            afs::crate_dir("vulkan_layer").join("layer/alvr_x86_64.json"),
            build_layout.vulkan_layer_manifest(),
        )
        .unwrap();

        // copy ffmpeg binaries
        if gpl {
            let lib_dir = &build_layout.openvr_driver_root_dir;
            let mut libavcodec_so = std::path::PathBuf::new();
            sh.create_dir(&lib_dir).unwrap();
            let _push_guard = sh.push_dir(&lib_dir);
            for lib_path in sh
                .read_dir(afs::deps_dir().join("linux/ffmpeg/alvr_build/lib"))
                .unwrap()
                .into_iter()
                .filter(|path| path.file_name().unwrap().to_string_lossy().contains(".so."))
            {
                let src_so_file = lib_path.canonicalize().unwrap(); // canonicalize resolves symlinks.
                sh.copy_file(&src_so_file, ".").unwrap();
                // Shell::copy_file does not handle symlinks so we must recreate them.
                if lib_path.is_symlink() {
                    assert!(lib_dir.join(src_so_file.file_name().unwrap()).exists());
                    let so_file = std::path::Path::new(src_so_file.file_name().unwrap());
                    let so_file_symlink = std::path::Path::new(lib_path.file_name().unwrap());
                    command::make_symlink(&sh, so_file, so_file_symlink).unwrap();
                }
                let lib_filename = src_so_file.file_name().unwrap();
                if lib_filename.to_string_lossy().starts_with("libavcodec.so") {
                    libavcodec_so = src_so_file;
                }
            }
            // copy ffmpeg shared lib dependencies.
            for solib in ["libx264.so", "libx265.so"] {
                let src_libs = dependencies::find_resolved_so_paths(&libavcodec_so, solib);
                if !src_libs.is_empty() {
                    let src_lib = src_libs.first().unwrap();
                    sh.copy_file(&src_lib, ".").unwrap();
                }
            }
        }
    }

    // copy static resources
    {
        // copy dashboard
        command::copy_recursive(
            &sh,
            &afs::workspace_dir().join("dashboard"),
            &build_layout.dashboard_dir(),
        )
        .unwrap();

        // copy presets
        command::copy_recursive(
            &sh,
            &afs::crate_dir("xtask").join("resources/presets"),
            &build_layout.presets_dir(),
        )
        .unwrap();

        // copy driver manifest
        sh.copy_file(
            afs::crate_dir("xtask").join("resources/driver.vrdrivermanifest"),
            &build_layout.openvr_driver_manifest(),
        )
        .unwrap();
    }

    // build experiments
    if experiments {
        command::copy_recursive(
            &sh,
            &afs::workspace_dir().join("experiments/gui/resources/languages"),
            &build_layout.static_resources_dir.join("languages"),
        )
        .unwrap();

        let _push_guard = sh.push_dir(afs::workspace_dir().join("experiments/launcher"));
        cmd!(sh, "cargo build {common_flags_ref...}").run().unwrap();
        sh.copy_file(
            artifacts_dir.join(afs::exec_fname("launcher")),
            build_layout
                .executables_dir
                .join(afs::exec_fname("experimental_launcher")),
        )
        .unwrap();
    }
}

pub fn build_client_lib(is_release: bool) {
    let sh = Shell::new().unwrap();

    let build_dir = afs::build_dir().join("alvr_client_core");
    sh.create_dir(&build_dir).unwrap();

    let mut flags = vec![];
    if is_release {
        flags.push("--release");
    }
    let flags_ref = &flags;

    let _push_guard = sh.push_dir(afs::crate_dir("client_core"));

    cmd!(
        sh,
        "cargo ndk -t arm64-v8a -o {build_dir} build {flags_ref...}"
    )
    .run()
    .unwrap();

    let out = build_dir.join("alvr_client_core.h");
    cmd!(sh, "cbindgen --output {out}").run().unwrap();
}

pub fn build_quest_client(is_release: bool) {
    let sh = Shell::new().unwrap();

    build_client_lib(is_release);

    let is_nightly = version::version().contains("nightly");
    let is_release = if is_nightly { false } else { is_release };

    let package_type = if is_nightly { "Nightly" } else { "Stable" };
    let build_type = if is_release { "release" } else { "debug" };

    let build_task = format!("assemble{package_type}{build_type}");

    let client_dir = afs::workspace_dir().join("android");

    const ARTIFACT_NAME: &str = "alvr_client_oculus_quest";

    let _push_guard = sh.push_dir(&client_dir);
    if cfg!(windows) {
        cmd!(sh, "cmd /C gradlew.bat {build_task}").run().unwrap();
    } else {
        cmd!(sh, "bash ./gradlew {build_task}").run().unwrap();
    };

    sh.create_dir(&afs::build_dir().join(ARTIFACT_NAME))
        .unwrap();
    sh.copy_file(
        client_dir
            .join("app/build/outputs/apk")
            .join(package_type)
            .join(build_type)
            .join(format!("app-{package_type}-{build_type}.apk")),
        afs::build_dir()
            .join(ARTIFACT_NAME)
            .join(format!("{ARTIFACT_NAME}.apk")),
    )
    .unwrap();
}

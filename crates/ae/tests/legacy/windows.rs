#![cfg(windows)]

use super::*;
use std::fs;

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

struct Machine {
    _root: tempfile::TempDir,
    local: PathBuf,
    roaming: PathBuf,
    home: PathBuf,
    street: PathBuf,
}

impl Machine {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().to_owned();
        Self {
            _root: root,
            local: path.join("Local"),
            roaming: path.join("Roaming"),
            home: path.join("home"),
            street: path.join("Local/Artisan Street"),
        }
    }

    fn old(&self) -> Old {
        Old::windows(
            &self.local,
            &self.roaming,
            &self.home,
            &self.street.join("Editor"),
        )
        .unwrap()
    }

    fn moves(&self, old: &Old, apps: &[App]) -> Vec<DataMove> {
        let mut moves = Vec::new();
        if apps.contains(&App::Forge) {
            moves.push(DataMove {
                data: Data::Forge,
                sources: old.forge_data.clone(),
                destination: self.street.join("Forge/data"),
            });
        }
        if apps.contains(&App::Editor) {
            moves.push(DataMove {
                data: Data::Editor,
                sources: old.editor_data.clone(),
                destination: self.street.join("Editor/data"),
            });
        }
        moves
    }

    fn migrate(&self, done: &[App]) -> Vec<String> {
        let old = self.old();
        let moves = self.moves(&old, done);
        let work = Work {
            old: &old,
            done,
            moves: &moves,
        };
        let notes = work.move_data().unwrap();
        work.retire().unwrap();
        notes
    }
}

#[test]
fn the_earlier_windows_install_moves_its_data_into_artisan_street_and_goes() {
    let machine = Machine::new();
    let release = machine.local.join("Artisan");
    let programs = [
        release.join("bin/ae.exe"),
        release.join("versions/0.3.0/ae.exe"),
        release.join("versions/0.3.0/artisan-editor.exe"),
        release.join("current/ae.exe"),
        machine.home.join("Desktop/Artisan Editor.lnk"),
    ];
    for program in &programs {
        write(program, "old");
    }
    write(&release.join("forge/forge.sqlite3"), "threads");
    write(&release.join("forge/forge.lock"), "held by the old Forge");
    write(
        &machine.roaming.join("Artisan/editor/hosts.json"),
        "[\"https://127.0.0.1:47901\"]",
    );

    let moved = machine.migrate(&[App::Ae, App::Forge, App::Editor]);

    assert_eq!(moved.len(), 2, "{moved:?}");
    assert_eq!(
        fs::read_to_string(machine.street.join("Forge/data/forge.sqlite3")).unwrap(),
        "threads"
    );
    assert!(
        !machine.street.join("Forge/data/forge.lock").exists(),
        "the old Forge's lock is not carried over"
    );
    assert_eq!(
        fs::read_to_string(machine.street.join("Editor/data/hosts.json")).unwrap(),
        "[\"https://127.0.0.1:47900\"]",
        "the Editor points at the one Forge"
    );
    for program in &programs {
        assert!(!program.exists(), "{} stayed", program.display());
    }
    assert!(!release.exists(), "the emptied install folder goes");
    assert!(!machine.roaming.join("Artisan").exists());
}

#[test]
fn installing_forge_alone_on_windows_leaves_the_earlier_editor_installed() {
    let machine = Machine::new();
    let release = machine.local.join("Artisan");
    let dev = machine.local.join("Artisan Dev");
    let ae = release.join("bin/ae.exe");
    let editors = [
        release.join("versions/0.3.0/artisan-editor.exe"),
        dev.join("artisan-editor.exe"),
        dev.join("launch.cmd"),
        machine.home.join("Desktop/Artisan Editor.lnk"),
    ];
    write(&ae, "old");
    for editor in &editors {
        write(editor, "old");
    }
    write(&dev.join("editor/settings.json"), "dev settings");

    machine.migrate(&[App::Ae, App::Forge]);

    assert!(!ae.exists(), "the old ae went with Forge and ae");
    for kept in &editors {
        assert!(kept.exists(), "{} belongs to the Editor", kept.display());
    }
    assert_eq!(
        fs::read_to_string(dev.join("editor/settings.json")).unwrap(),
        "dev settings",
        "the Editor's settings stay with the Editor"
    );
}

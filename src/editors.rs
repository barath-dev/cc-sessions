//! Code editors a session's folder can be opened in.

use std::path::{Path, PathBuf};

use crate::os;
use crate::util::{err, tilde, Result};

pub struct Editor {
    pub id: &'static str,
    pub name: &'static str,
    /// Commands that open a folder when given one, as installed on PATH.
    commands: &'static [&'static str],
    /// The app's names on a Mac, used when its command is not on PATH.
    apps: &'static [&'static str],
}

const EDITORS: &[Editor] = &[
    Editor { id: "vscode", name: "VS Code", commands: &["code"], apps: &["Visual Studio Code.app"] },
    Editor { id: "cursor", name: "Cursor", commands: &["cursor"], apps: &["Cursor.app"] },
    Editor { id: "windsurf", name: "Windsurf", commands: &["windsurf"], apps: &["Windsurf.app"] },
    Editor { id: "zed", name: "Zed", commands: &["zed", "zeditor"], apps: &["Zed.app"] },
    Editor { id: "vscodium", name: "VSCodium", commands: &["codium"], apps: &["VSCodium.app"] },
    Editor { id: "sublime", name: "Sublime Text", commands: &["subl"], apps: &["Sublime Text.app"] },
    Editor {
        id: "intellij",
        name: "IntelliJ IDEA",
        commands: &["idea"],
        apps: &["IntelliJ IDEA.app", "IntelliJ IDEA Ultimate.app", "IntelliJ IDEA CE.app"],
    },
    Editor { id: "webstorm", name: "WebStorm", commands: &["webstorm"], apps: &["WebStorm.app"] },
    Editor {
        id: "pycharm",
        name: "PyCharm",
        commands: &["pycharm", "charm"],
        apps: &["PyCharm.app", "PyCharm Professional Edition.app", "PyCharm CE.app"],
    },
    Editor { id: "goland", name: "GoLand", commands: &["goland"], apps: &["GoLand.app"] },
    Editor { id: "rustrover", name: "RustRover", commands: &["rustrover"], apps: &["RustRover.app"] },
    Editor { id: "android-studio", name: "Android Studio", commands: &["studio"], apps: &["Android Studio.app"] },
];

/// Where an editor can be started from: its command on PATH, else its app on a Mac.
fn locate(editor: &Editor) -> Option<PathBuf> {
    editor.commands.iter().find_map(|c| os::which(c)).or_else(|| os::find_app(editor.apps))
}

/// The editors on this computer, in the table's order.
pub fn installed() -> Vec<&'static Editor> {
    EDITORS.iter().filter(|e| locate(e).is_some()).collect()
}

/// Open a folder in an editor. Returns the editor's name.
pub fn open(id: &str, folder: &Path) -> Result<&'static str> {
    let Some(editor) = EDITORS.iter().find(|e| e.id == id) else {
        return err(format!("Unknown editor '{id}'."));
    };
    let Some(program) = locate(editor) else {
        return err(format!("{} was not found on this computer.", editor.name));
    };
    if !folder.is_dir() {
        return err(format!("Its folder {} no longer exists.", tilde(&folder.to_string_lossy())));
    }
    os::open_folder_in(&program, folder)?;
    Ok(editor.name)
}

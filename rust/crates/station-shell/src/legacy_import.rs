//! Command-line entry for importing a closed Electron Station profile.

use std::ffi::OsString;
use std::io;
use std::path::PathBuf;

pub(crate) fn default_database_path() -> Option<PathBuf> {
    dirs::config_dir().map(|directory| directory.join("Stationv2").join("db").join("station.db"))
}

pub(crate) fn run(mut args: impl Iterator<Item = OsString>) -> Option<io::Result<()>> {
    if args.next()? != "--import-electron" {
        return None;
    }
    let source = args
        .next()
        .map(PathBuf::from)
        .or_else(default_database_path);
    if args.next().is_some() {
        return Some(Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "usage: station-shell --import-electron [path to station.db]",
        )));
    }
    let Some(source) = source else {
        return Some(Err(io::Error::new(
            io::ErrorKind::NotFound,
            "pass the path to your legacy station.db",
        )));
    };
    Some(crate::instance::claim().and_then(|primary| {
        if !primary {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "quit the native Station app before importing",
            ));
        }
        session_store::legacy::import(&source, &crate::session::path())
            .map(|summary| {
                println!(
                    "Imported {} apps, {} tabs, and {} Quick Switcher links. Skipped {} apps, {} tabs, and {} favorites.",
                    summary.applications,
                    summary.tabs,
                    summary.favorites,
                    summary.skipped_applications,
                    summary.skipped_tabs,
                    summary.skipped_favorites,
                );
            })
            .map_err(|error| {
                io::Error::new(error.kind(), format!("{}: {error}", source.display()))
            })
    }))
}

//! `delete`: moves a recording to the system Trash. Undo needs the in-memory `Trashed`, so it
//! cannot span processes: the files stay in the Trash for the user to put back by hand.

use dory::Config;

use crate::args::DeleteArgs;
use crate::open;

pub fn run(config: &Config, args: &DeleteArgs) -> Result<(), String> {
    let dory = open(config, false)?;
    let trashed = dory.library().trash(&args.id).map_err(crate::chain)?;
    println!(
        "moved {} ({}) to the Trash",
        trashed.id(),
        trashed.title().unwrap_or("untitled")
    );
    Ok(())
}

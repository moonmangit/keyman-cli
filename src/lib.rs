//! Keyman's core: the master file, saving `.env` back into it, and shared
//! formatting helpers. The `keyman` binary and the desktop app both use it.

pub mod envfile;
pub mod master;
pub mod save;
pub mod ui;

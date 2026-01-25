//! Generate random agent names.

use crate::core::names::generate_name;

/// Run the generate-name command.
///
/// Prints a random kebab-case name to stdout.
pub fn run() {
    println!("{}", generate_name());
}

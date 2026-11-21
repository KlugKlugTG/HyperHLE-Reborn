/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
// Allow the crate to have a non-snake-case name (touchHLE).
// This also allows items in the crate to have non-snake-case names.
#![allow(non_snake_case)]

fn main() -> std::process::ExitCode {
    match touchHLE::main(std::env::args()) {
        Ok(termination) => {
            std::process::ExitCode::from(termination.map_or(0, |result| result.exit_code()) as u8)
        }
        Err(error) => {
            eprintln!("HyperHLE failed: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

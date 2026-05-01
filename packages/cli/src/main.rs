//! Tally CLI
//!
//! End-user CLI for managing Tally teams, role packs, and runtime
//! deployments. Distributed as a standalone binary.
//!
//! Status: skeleton. Real implementation comes in Phase 1B.

fn main() {
    println!("Tally CLI v{}", env!("CARGO_PKG_VERSION"));
    println!("Status: skeleton. Real CLI coming in Phase 1B.");
    println!("See: https://github.com/nicholasraimbault/tally");
}

#[cfg(test)]
mod tests {
    #[test]
    fn smoke() {
        // Just verify the binary's main module compiles
        assert_eq!(2 + 2, 4);
    }
}

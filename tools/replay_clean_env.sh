# Sourced by replay_compare.sh: the environment cargo and rustc run in for every comparison build.
#
# `env -i` with only PATH, HOME and those of RUSTUP_HOME, CARGO_HOME, TMPDIR that are set, and
# CARGO_ENCODED_RUSTFLAGS set to empty. Cargo reads compiler flags from CARGO_ENCODED_RUSTFLAGS
# first, then RUSTFLAGS, then `target.<triple>.rustflags`, `target.<cfg>.rustflags` and
# `build.rustflags` of any config file, and uses the first that is present: an empty
# CARGO_ENCODED_RUSTFLAGS is present, so it ends the search with no flags and a user's
# `target.<triple>.rustflags = ["-C", "target-cpu=native"]` is not read. (`--config
# build.rustflags=[]` alone would lose to the target tables.) The caller adds the variables it
# sets itself (CARGO_BUILD_JOBS, CARGO_TARGET_DIR).
replay_clean_env() {
    clean_env_prefix=(env -i "PATH=$PATH" "HOME=${HOME-}" "CARGO_ENCODED_RUSTFLAGS=")
    [ -z "${RUSTUP_HOME+x}" ] || clean_env_prefix+=("RUSTUP_HOME=$RUSTUP_HOME")
    [ -z "${CARGO_HOME+x}" ] || clean_env_prefix+=("CARGO_HOME=$CARGO_HOME")
    [ -z "${TMPDIR+x}" ] || clean_env_prefix+=("TMPDIR=$TMPDIR")
}

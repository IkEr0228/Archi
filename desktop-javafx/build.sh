#!/bin/sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
root=$(CDPATH= cd -- "$script_dir/.." && pwd)
export CARGO_TARGET_DIR="$root/target/linux"
cache="$root/.cache/javafx"
classes="$script_dir/build/linux/classes"
mkdir -p "$cache" "$classes"

fetch() {
    path=$1
    sha=$2
    file="$cache/${path##*/}"
    if [ ! -f "$file" ]; then
        curl --fail --location --silent --show-error "https://repo.maven.apache.org/maven2/$path" -o "$file"
    fi
    actual=$(sha256sum "$file" | awk '{print $1}')
    [ "$actual" = "$sha" ] || { echo "Dependency checksum mismatch: $file" >&2; exit 1; }
}

fetch 'org/openjfx/javafx-base/21.0.10/javafx-base-21.0.10-linux.jar' 'e31ea9955b584648d8e163778558281ddfb92958113c6a1e3c056113b3b65308'
fetch 'org/openjfx/javafx-graphics/21.0.10/javafx-graphics-21.0.10-linux.jar' 'f88de61d58a05f0ea30e0e4501e202f3fc9dd99172247bc5e1660929a6b3b836'
fetch 'org/openjfx/javafx-controls/21.0.10/javafx-controls-21.0.10-linux.jar' '8489a50c15957d83e83d4a9933ff9b5c91aca2aa645bc9b46882c39a63f8820c'
fetch 'com/google/code/gson/gson/2.13.2/gson-2.13.2.jar' 'dd0ce1b55a3ed2080cb70f9c655850cda86c206862310009dcb5e5c95265a5e0'

gson="$cache/gson-2.13.2.jar"

java_bin=
case "${JAVA_HOME-}" in
    ''|*:*|*\\*) ;;
    *) [ -x "$JAVA_HOME/bin/javac" ] && java_bin="$JAVA_HOME/bin" ;;
esac
java_bin=${java_bin:-$(dirname "$(command -v javac)")}
module_path="$cache/javafx-base-21.0.10-linux.jar:$cache/javafx-graphics-21.0.10-linux.jar:$cache/javafx-controls-21.0.10-linux.jar"
find "$script_dir/src" -name '*.java' -exec printf '"%s"\n' {} \; > "$script_dir/build/sources.list"
"$java_bin/javac" --release 21 --module-path "$module_path" --add-modules javafx.controls -cp "$gson" -d "$classes" @"$script_dir/build/sources.list"
cp "$script_dir"/resources/* "$classes"/

run=false
test=false
smoke=false
release=false
archive=
while [ "$#" -gt 0 ]; do
    case $1 in
        --run) run=true ;;
        --test) test=true ;;
        --smoke) smoke=true ;;
        --release) release=true ;;
        --archive) shift; archive=${1-} ;;
        *) echo "Usage: $0 [--run] [--test] [--smoke] [--release] [--archive FILE]" >&2; exit 2 ;;
    esac
    shift
done

if $test || $smoke; then
    find "$script_dir/test" -name '*.java' -exec printf '"%s"\n' {} \; > "$script_dir/build/tests.list"
    "$java_bin/javac" --release 21 --module-path "$module_path" --add-modules javafx.controls -cp "$gson:$classes" -d "$classes" @"$script_dir/build/tests.list"
fi

if $run || $test || $smoke || $release; then
    profile=debug
    if $release; then profile=release; fi
    if $release; then
        CARGO_INCREMENTAL=0 cargo build --locked --manifest-path "$root/Cargo.toml" -p archi-worker --release
    else
        CARGO_INCREMENTAL=0 cargo build --locked --manifest-path "$root/Cargo.toml" -p archi-worker
    fi
    worker="$CARGO_TARGET_DIR/$profile/archi-worker"
    run_java() {
        "$java_bin/java" -ea --module-path "$module_path" --add-modules javafx.controls -cp "$gson:$classes" "-Darchi.worker=$worker" "$@"
    }
    if $test; then
        run_java archi.WorkerTest
    fi
    if $smoke; then
        run_java "-Darchi.smoke.output=$script_dir/build" archi.FxSmokeTest
        run_java "-Darchi.smoke.output=$script_dir/build" archi.FxSmokeTest "$root/crates/archi-core/tests/fixtures/comment-hpw-password.rar"
    fi
fi

if $run; then
    if [ -n "$archive" ]; then
        "$java_bin/java" --module-path "$module_path" --add-modules javafx.controls -cp "$gson:$classes" "-Darchi.worker=$worker" archi.Launcher "$archive"
    else
        "$java_bin/java" --module-path "$module_path" --add-modules javafx.controls -cp "$gson:$classes" "-Darchi.worker=$worker" archi.Launcher
    fi
fi

if $test || $smoke; then
    "$java_bin/java" -ea --module-path "$module_path" --add-modules javafx.controls -cp "$gson:$classes" archi.ModelTest
fi

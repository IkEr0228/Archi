#!/bin/sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
root=$(CDPATH= cd -- "$script_dir/.." && pwd)
destination=${1:-$root/dist-release/javafx-linux}
case "$destination" in /*) ;; *) destination="$(CDPATH= cd -- "$(dirname -- "$destination")" && pwd)/$(basename -- "$destination")" ;; esac
[ ! -e "$destination/Archi" ] || { echo "Archi output already exists: choose another destination" >&2; exit 1; }

sh "$script_dir/build.sh" --release
cache="$root/.cache/javafx"
classes="$script_dir/build/linux/classes"
worker="$root/target/linux/release/archi-worker"
[ -x "$worker" ] || { echo 'Release worker build missing' >&2; exit 1; }
java_bin=
case "${JAVA_HOME-}" in
    ''|*:*|*\\*) ;;
    *) [ -x "$JAVA_HOME/bin/jpackage" ] && java_bin="$JAVA_HOME/bin" ;;
esac
java_bin=${java_bin:-$(dirname "$(command -v jpackage)")}
scratch=$(mktemp -d "$script_dir/build/package.XXXXXX")
trap 'rm -rf "$scratch"' EXIT
input="$scratch/input"
runtime="$scratch/runtime"
staging="$scratch/classes"
mkdir -p "$input" "$staging/archi" "$destination"

cat > "$scratch/MANIFEST.MF" <<'EOF'
Manifest-Version: 1.0
Main-Class: archi.Launcher
Class-Path: gson-2.13.2.jar

EOF
for source in "$script_dir"/src/archi/*.java; do
    name=$(basename "$source" .java)
    find "$classes/archi" -maxdepth 1 -name "$name*.class" -exec cp {} "$staging/archi/" \;
done
cp "$classes/archi.css" "$staging/"
mkdir -p "$input/licenses"
cp "$root/LICENSE" "$input/licenses/Archi-MIT.txt"
cp "$root/vendor/sevenz-rust2/LICENSE" "$input/licenses/sevenz-rust2-Apache-2.0.txt"
"$java_bin/jar" --create --file "$input/archi.jar" --manifest "$scratch/MANIFEST.MF" -C "$staging" .
cp "$cache/gson-2.13.2.jar" "$worker" "$input/"
java_home=$(dirname -- "$(dirname -- "$(readlink -f "$java_bin/java")")")
jmods="$java_home/jmods"
modules="$cache/javafx-base-21.0.10-linux.jar:$cache/javafx-graphics-21.0.10-linux.jar:$cache/javafx-controls-21.0.10-linux.jar:$jmods"
"$java_bin/jlink" --module-path "$modules" --add-modules javafx.controls,java.desktop,java.logging,jdk.unsupported --strip-debug --no-header-files --no-man-pages --output "$runtime"
"$java_bin/jpackage" --type app-image --name Archi --app-version 0.5.0 --vendor Archi --input "$input" --main-jar archi.jar --main-class archi.Launcher --runtime-image "$runtime" --java-options '--add-modules=javafx.controls' --dest "$destination"
echo "Portable application: $destination/Archi/bin/Archi"
echo 'Java runtime included.'

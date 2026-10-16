#!/usr/bin/env bash
# Make the reference GNU grep: the grep and PCRE2 of Ubuntu 24.04 (grep 3.11, PCRE2 10.42),
# which are what the CI runner has, whatever the machine running this is.
#
#   tools/ref-grep.sh          print the path of the reference `grep` wrapper (made if needed)
#
# The packages come from the Ubuntu archive. The signed index (InRelease, checked against the
# archive keyring) gives the hash of the package list, and the package list gives the hash of
# each package, so what runs is what Ubuntu built. Nothing is installed: the packages are
# unpacked under target/reference/grep and the wrapper runs that grep with that PCRE2.
# The wrapper is named `grep` so that the messages carry the same `grep:` prefix.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
dir="$root/target/reference/grep"
wrapper="$dir/bin/grep"
mirror=${UBUNTU_MIRROR:-http://archive.ubuntu.com/ubuntu}
suite=noble
packages=(grep=3.11-4build1 libpcre2-8-0=10.42-4ubuntu2)

if [ -x "$wrapper" ] && [ -f "$dir/.done" ]; then
  echo "$wrapper"
  exit 0
fi

keyring=/usr/share/keyrings/ubuntu-archive-keyring.gpg
[ -f "$keyring" ] || { echo "ref-grep: $keyring is needed to check the packages (package ubuntu-keyring)" >&2; exit 1; }
command -v gpgv > /dev/null || { echo "ref-grep: gpgv is needed to check the packages" >&2; exit 1; }

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
fetch() { curl -sSfL --retry 3 -m 120 -o "$2" "$1"; }

fetch "$mirror/dists/$suite/InRelease" "$work/InRelease"
gpgv --keyring "$keyring" "$work/InRelease" > /dev/null 2>&1 || { echo "ref-grep: the signature of InRelease is not good" >&2; exit 1; }
fetch "$mirror/dists/$suite/main/binary-amd64/Packages.gz" "$work/Packages.gz"
sum=$(sha256sum "$work/Packages.gz" | cut -d' ' -f1)
rg -q "^ $sum +[0-9]+ main/binary-amd64/Packages.gz$" "$work/InRelease" 2> /dev/null \
  || awk -v s="$sum" '$1 == s && $3 == "main/binary-amd64/Packages.gz" { found = 1 } END { exit !found }' "$work/InRelease" \
  || { echo "ref-grep: Packages.gz does not match the signed index" >&2; exit 1; }

rm -rf "$dir"
mkdir -p "$dir/root" "$dir/bin"
for spec in "${packages[@]}"; do
  name=${spec%%=*}
  version=${spec#*=}
  # Filename and SHA256 of exactly that package and version.
  entry=$(zcat "$work/Packages.gz" | awk -v n="$name" -v v="$version" '
    BEGIN { RS = ""; FS = "\n" }
    { p = ""; ver = ""; f = ""; h = ""
      for (i = 1; i <= NF; i++) {
        if ($i ~ /^Package: /) p = substr($i, 10)
        else if ($i ~ /^Version: /) ver = substr($i, 10)
        else if ($i ~ /^Filename: /) f = substr($i, 11)
        else if ($i ~ /^SHA256: /) h = substr($i, 9)
      }
      if (p == n && ver == v && !done) { print f " " h; done = 1 }
    }')
  [ -n "$entry" ] || { echo "ref-grep: $spec is not in the index of $suite" >&2; exit 1; }
  file=${entry% *}
  hash=${entry#* }
  fetch "$mirror/$file" "$work/pkg.deb"
  [ "$(sha256sum "$work/pkg.deb" | cut -d' ' -f1)" = "$hash" ] || { echo "ref-grep: $spec does not match its hash" >&2; exit 1; }
  dpkg-deb -x "$work/pkg.deb" "$dir/root"
done

cat > "$wrapper" <<EOF
#!/bin/bash
# The reference grep (see tools/ref-grep.sh), run as \`grep\`.
LD_LIBRARY_PATH="$dir/root/usr/lib/x86_64-linux-gnu" exec -a grep "$dir/root/usr/bin/grep" "\$@"
EOF
chmod +x "$wrapper"
# egrep and fgrep run the same program under their own names.
for name in egrep fgrep; do
  sed "s|exec -a grep|exec -a $name|; s|usr/bin/grep|usr/bin/$name|" "$wrapper" > "$dir/bin/$name"
  chmod +x "$dir/bin/$name"
done
touch "$dir/.done"
echo "$wrapper"

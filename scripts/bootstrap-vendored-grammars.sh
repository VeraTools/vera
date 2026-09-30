#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${1:-$REPO_ROOT}"

checksum() {
    if command -v sha256sum >/dev/null; then
        sha256sum "$1" | cut -d ' ' -f 1
    else
        shasum -a 256 "$1" | cut -d ' ' -f 1
    fi
}

# Keep a verified cache; never publish a partial or unexpected upstream file.
TEMP_FILE=""
trap 'if [[ -n "$TEMP_FILE" ]]; then rm -f "$TEMP_FILE"; fi' EXIT
while read -r grammar repository revision file expected; do
    target="crates/tree-sitter-$grammar/$file"
    if [[ -f "$target" && "$(checksum "$target")" == "$expected" ]]; then
        continue
    fi
    mkdir -p "$(dirname "$target")"
    TEMP_FILE=$(mktemp "$target.tmp.XXXXXXXX")
    curl --fail --silent --show-error --location --connect-timeout 10 --max-time 120 \
        --retry 3 --retry-connrefused \
        "https://raw.githubusercontent.com/$repository/$revision/$file" -o "$TEMP_FILE"
    if [[ "$(checksum "$TEMP_FILE")" != "$expected" ]]; then
        echo "[bootstrap] Checksum mismatch: $target" >&2
        exit 1
    fi
    chmod 644 "$TEMP_FILE"
    mv -f "$TEMP_FILE" "$target"
    TEMP_FILE=""
done <<'GRAMMARS'
vue tree-sitter-grammars/tree-sitter-vue ce8011a414fdf8091f4e4071752efc376f4afb08 src/parser.c 770f6ec24e908a30adf26056cc20375d94e2918bc79c0bdff4331d1dbd5577ce
vue tree-sitter-grammars/tree-sitter-vue ce8011a414fdf8091f4e4071752efc376f4afb08 src/scanner.c 9c2147fe2de1ede71f3ef5fd3ba54d12dbb6c2f71bce5735358fdbd4f8d19a32
vue tree-sitter-grammars/tree-sitter-vue ce8011a414fdf8091f4e4071752efc376f4afb08 src/tag.h b639821160ac0e1a70d1dc2fe206380da945bd65be7a010ffdb90694a0fef2b8
vue tree-sitter-grammars/tree-sitter-vue ce8011a414fdf8091f4e4071752efc376f4afb08 src/tree_sitter/alloc.h b29c1c9fb7cc82f58c84b376df1297d6e2737a1d655fd356db0859e3c29c2fea
vue tree-sitter-grammars/tree-sitter-vue ce8011a414fdf8091f4e4071752efc376f4afb08 src/tree_sitter/array.h 5bdf6ed1a78e3409fd443e085ca967a64c188a5d082aaf7f819bccd53a471c94
vue tree-sitter-grammars/tree-sitter-vue ce8011a414fdf8091f4e4071752efc376f4afb08 src/tree_sitter/parser.h 180b893c8734778fd32f372dfbc27bd6ad1cd2221f26150b31256ff6716320d2
scss serenadeai/tree-sitter-scss c478c6868648eff49eb04a4df90d703dc45b312a src/parser.c 40fc07e8edbe45145cfe96ea47293336826aa38267feb17ad302125c4e3c4307
scss serenadeai/tree-sitter-scss c478c6868648eff49eb04a4df90d703dc45b312a src/scanner.c fba18f82eb63f7d51545afabfd815e50921ff7388a48cc0393512b41ad2ed9d4
scss serenadeai/tree-sitter-scss c478c6868648eff49eb04a4df90d703dc45b312a src/tree_sitter/parser.h 05729ff0c43ba3aebd8429a71e13c25a5dc7944e0eb725438bc98fda44573285
dockerfile camdencheek/tree-sitter-dockerfile 971acdd908568b4531b0ba28a445bf0bb720aba5 src/parser.c 4265eb2433dd67d270e4a901312d7050f686e6a45dca9e7879fdd2d49e7ed501
dockerfile camdencheek/tree-sitter-dockerfile 971acdd908568b4531b0ba28a445bf0bb720aba5 src/scanner.c 1080c2eb2ac41f102974e009cb62f644d9d638dd2468d0a700674d07d346fde7
dockerfile camdencheek/tree-sitter-dockerfile 971acdd908568b4531b0ba28a445bf0bb720aba5 src/tree_sitter/parser.h a1f6ef161fbaf48a0e10fca90ef5290a062462b307b3898aa562993853b9f80a
astro virchau13/tree-sitter-astro 213f6e6973d9b456c6e50e86f19f66877e7ef0ee src/parser.c 64bba63bd3f101007ce4dffae94ce3e38b2793f40ddf39db101b7ecd5e612c29
astro virchau13/tree-sitter-astro 213f6e6973d9b456c6e50e86f19f66877e7ef0ee src/scanner.c 9d7233e8ecdf695c849eb7c48c724ec8b26f06bc1543f499cc0ae95e28df6817
astro virchau13/tree-sitter-astro 213f6e6973d9b456c6e50e86f19f66877e7ef0ee src/tag.h 67b9e557aa5092a8719e16958a445b748f8168527715704209148d0d63797fc0
astro virchau13/tree-sitter-astro 213f6e6973d9b456c6e50e86f19f66877e7ef0ee src/tree_sitter/alloc.h 253b44a7b4313a7afd0c505c2fc6e7ce4b8e78955ebf4be3ea000532ec060673
astro virchau13/tree-sitter-astro 213f6e6973d9b456c6e50e86f19f66877e7ef0ee src/tree_sitter/array.h 4ff743903dc46f5db6aa54f31c6b4d160a8a9779e5b2ab1ee59ae7ebcd850ea1
astro virchau13/tree-sitter-astro 213f6e6973d9b456c6e50e86f19f66877e7ef0ee src/tree_sitter/parser.h 8e819abdeba5866bf1aaf6e7b97522241c903d94a18395c6a75ed02984270ee8
GRAMMARS

echo "[bootstrap] Vendored grammar checksums verified"

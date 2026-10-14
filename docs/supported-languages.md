# Supported languages

Vera recognizes 65 languages and file formats. It picks the language from the file extension, or from the full filename for files like `Dockerfile`, `Makefile`, `CMakeLists.txt`, and `nginx.conf`. Matching is case-insensitive.

Files with any other extension are still indexed, as plain text.

## How files are chunked

The language decides how a file is split into search results:

| Chunking | What you get |
|---|---|
| Symbols | Vera parses the file with tree-sitter and returns functions, classes, structs, methods, and similar definitions as separate results. Long symbols are split. |
| Whole file | The file is one result, named after the file. Files larger than `indexing.max_chunk_bytes` are split at line boundaries. |
| Sections | One result per heading. |
| Line windows | Overlapping 50-line windows. Also used for files whose language Vera does not recognize. |

Use the value in the `--lang` column to filter results, for example `vera search "retry logic" --lang cpp`.

## Systems and compiled languages

| Language | Extensions | `--lang` | Chunking |
|---|---|---|---|
| C | `.c`, `.h` | `c` | Symbols |
| C++ | `.cpp`, `.cc`, `.cxx`, `.hpp`, `.hxx`, `.hh` | `cpp` | Symbols |
| Rust | `.rs` | `rust` | Symbols |
| Go | `.go` | `go` | Symbols |
| Swift | `.swift` | `swift` | Symbols |
| Objective-C | `.m`, `.mm` | `objectivec` | Symbols |
| Zig | `.zig` | `zig` | Symbols |
| D | `.d`, `.di` | `d` | Symbols |
| Fortran | `.f`, `.f90`, `.f95` | `fortran` | Symbols |

## JVM and .NET

| Language | Extensions | `--lang` | Chunking |
|---|---|---|---|
| Java | `.java` | `java` | Symbols |
| Kotlin | `.kt`, `.kts` | `kotlin` | Symbols |
| Scala | `.scala`, `.sc` | `scala` | Symbols |
| Groovy | `.groovy` | `groovy` | Symbols |
| Clojure | `.clj`, `.cljs`, `.cljc` | `clojure` | Symbols |
| C# | `.cs` | `csharp` | Symbols |
| F# | `.fs`, `.fsi`, `.fsx` | `fsharp` | Symbols |

## Scripting and application languages

| Language | Extensions | `--lang` | Chunking |
|---|---|---|---|
| Python | `.py`, `.pyi` | `python` | Symbols |
| Ruby | `.rb` | `ruby` | Symbols |
| PHP | `.php` | `php` | Symbols |
| Perl | `.pl`, `.pm` | `perl` | Symbols |
| Lua | `.lua` | `lua` | Symbols |
| Luau | `.luau` | `luau` | Symbols |
| R | `.r` | `r` | Symbols |
| Julia | `.jl` | `julia` | Symbols |
| MATLAB | `.mlx` | `matlab` | Symbols |
| Dart | `.dart` | `dart` | Symbols |

`.m` files are treated as Objective-C, not MATLAB.

## Functional languages

| Language | Extensions | `--lang` | Chunking |
|---|---|---|---|
| Haskell | `.hs` | `haskell` | Line windows |
| OCaml | `.ml`, `.mli` | `ocaml` | Symbols |
| Elixir | `.ex`, `.exs` | `elixir` | Symbols |
| Erlang | `.erl`, `.hrl` | `erlang` | Symbols |
| Common Lisp | `.lisp`, `.cl`, `.lsp` | `commonlisp` | Symbols |
| Scheme | `.scm`, `.ss` | `scheme` | Symbols |
| Racket | `.rkt` | `racket` | Symbols |
| Nix | `.nix` | `nix` | Symbols |

Haskell files are indexed as line windows, so they do not produce symbols or entries for `vera references`.

## Web and frontend

| Language | Extensions | `--lang` | Chunking |
|---|---|---|---|
| TypeScript | `.ts`, `.tsx` | `typescript` | Symbols |
| JavaScript | `.js`, `.jsx`, `.mjs`, `.cjs` | `javascript` | Symbols |
| HTML | `.html`, `.htm` | `html` | Symbols (elements) |
| CSS | `.css` | `css` | Symbols (rules) |
| SCSS | `.scss` | `scss` | Symbols |
| Vue | `.vue` | `vue` | Symbols |
| Svelte | `.svelte` | `svelte` | Symbols |
| Astro | `.astro` | `astro` | Symbols |
| Elm | `.elm` | `elm` | Symbols |
| GraphQL | `.graphql`, `.gql` | `graphql` | Symbols |

## Shell

| Language | Extensions | `--lang` | Chunking |
|---|---|---|---|
| Bash | `.sh`, `.bash` | `bash` | Symbols |
| Zsh | `.zsh` | `zsh` | Symbols |
| Fish | `.fish` | `fish` | Symbols |
| PowerShell | `.ps1`, `.psm1` | `powershell` | Symbols |

## Shaders

| Language | Extensions | `--lang` | Chunking |
|---|---|---|---|
| GLSL | `.glsl`, `.vert`, `.frag`, `.geom`, `.comp`, `.tesc`, `.tese` | `glsl` | Symbols |
| HLSL | `.hlsl`, `.hlsli`, `.fx` | `hlsl` | Symbols |

## Schema, infrastructure, and build files

| Language | Extensions or filenames | `--lang` | Chunking |
|---|---|---|---|
| SQL | `.sql` | `sql` | Symbols |
| HCL and Terraform | `.tf`, `.hcl` | `hcl` | Symbols |
| Protocol Buffers | `.proto` | `protobuf` | Symbols |
| Prisma | `.prisma` | `prisma` | Symbols |
| XML | `.xml`, `.xsl`, `.xsd`, `.svg` | `xml` | Symbols (elements) |
| Dockerfile | `Dockerfile` | `dockerfile` | Whole file |
| CMake | `.cmake`, `CMakeLists.txt` | `cmake` | Whole file |
| Makefile | `Makefile`, `GNUmakefile` | `makefile` | Whole file |
| Nginx | `.nginx`, `nginx.conf` | `nginx` | Whole file |
| INI | `.ini`, `.cfg`, `.conf` | `ini` | Whole file |

## Data and documentation

| Format | Extensions | `--lang` | Chunking |
|---|---|---|---|
| TOML | `.toml` | `toml` | Whole file |
| YAML | `.yaml`, `.yml` | `yaml` | Whole file |
| JSON | `.json` | `json` | Whole file |
| Markdown | `.md`, `.markdown` | `markdown` | Sections |
| reStructuredText | `.rst`, `.rst.inc` | `rst` | Sections |

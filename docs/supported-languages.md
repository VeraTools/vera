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

## C and C++ parser recovery

A `.h` file starts with the C grammar. If that parse has errors, Vera tries C++
and selects it only when the whole file parses cleanly and contains a C++-specific
construct (for example, a class, template, namespace, reference, or constructor
initializer). Valid C headers remain C. Explicit `.c` files never switch to C++.
The selected language is recorded in both chunks and index-health metadata.
Callers extracting references without chunking can pass the source path to
`parse_and_extract_references_for_path` for identical header and dialect
selection; the legacy pathless API retains its original signature.

For erroneous C/C++ parses, Vera also retries a byte- and line-preserving view
of the source for these common constructs:

- `WINAPI`, `CALLBACK`, `APIENTRY`, `NTAPI`, `WINAPIV`, and `PASCAL` when the
  grammar places the annotation in an error node.
- Matched three-line `#ifdef __cplusplus` / `extern "C" {` / `#endif` wrappers
  and their closing wrappers. `#if defined(__cplusplus)` and
  `#if defined __cplusplus` are also recognized. The wrapper braces must match;
  conditional branches containing declarations are not removed.

Recovery is accepted only if the complete retry has no syntax errors. It does
not expand macros, select build configurations, or replace compiler-based
analysis. Other errors remain visible. Indexed content and line numbers always
come from the original source, including annotations and linkage guards.

Rebuild existing indexes with `vera index .` after upgrading to pick up parser
changes in unchanged files. `vera update .` alone skips unchanged source files.

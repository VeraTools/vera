# Docker

Vera publishes images that run the MCP server, or any other Vera command, in a container. All images are linux/amd64.

| Image | Base | Backend | For |
|---|---|---|---|
| `ghcr.io/veratools/vera:cpu` | `debian:trixie-slim` | Potion Code | Any machine |
| `ghcr.io/veratools/vera:cuda` | `nvidia/cuda:13.1.1-runtime` | Jina ONNX, CUDA | NVIDIA GPUs |
| `ghcr.io/veratools/vera:rocm` | `rocm/dev-ubuntu-24.04:6.4.4` | Jina ONNX, ROCm | AMD GPUs |
| `ghcr.io/veratools/vera:openvino` | `ubuntu:24.04` with OpenVINO | Jina ONNX, OpenVINO | Intel GPUs and iGPUs |

The `cpu`, `cuda`, `rocm`, and `openvino` tags follow the latest stable release. For repeatable deployments, pin a version tag such as `2.1.3-cpu`.

Each image sets `VERA_BACKEND`, which takes priority over a backend saved with `vera setup`. Pass `-e VERA_BACKEND=<backend>` to use a different one. If you have indexes built with a `cpu` image from before v2, rebuild them; see [v2 migration](migration-v2.md#docker-images).

## Run the MCP server

The container starts `vera mcp` by default. Mount your project at `/workspace` and make it the working directory with `-w`. Keep `-i` so the server can read from stdin.

CPU:

```bash
docker run --rm -i -v "$(pwd):/workspace" -w /workspace ghcr.io/veratools/vera:cpu
```

NVIDIA:

```bash
docker run --rm -i --gpus all -v "$(pwd):/workspace" -w /workspace ghcr.io/veratools/vera:cuda
```

AMD:

```bash
docker run --rm -i --device=/dev/kfd --device=/dev/dri -v "$(pwd):/workspace" -w /workspace ghcr.io/veratools/vera:rocm
```

Intel:

```bash
docker run --rm -i --device=/dev/dri -v "$(pwd):/workspace" -w /workspace ghcr.io/veratools/vera:openvino
```

## Configure an MCP client

Point the client at `docker` instead of `vera`:

```json
{
  "mcpServers": {
    "vera": {
      "command": "docker",
      "args": ["run", "--rm", "-i", "-v", "/path/to/project:/workspace", "-w", "/workspace", "ghcr.io/veratools/vera:cpu"]
    }
  }
}
```

For a GPU image, add its device flags to `args`, for example:

```json
{
  "mcpServers": {
    "vera": {
      "command": "docker",
      "args": ["run", "--rm", "-i", "--gpus", "all", "-v", "/path/to/project:/workspace", "-w", "/workspace", "ghcr.io/veratools/vera:cuda"]
    }
  }
}
```

## Keep downloaded models between runs

On first use, the CPU image downloads Potion Code, and the GPU images download their ONNX models and runtime. They are stored in `/root/.local/share/vera/` inside the container. Mount a volume there so later runs reuse them:

```bash
docker run --rm -i \
  -v "$(pwd):/workspace" -w /workspace \
  -v vera-models:/root/.local/share/vera \
  ghcr.io/veratools/vera:cpu
```

Older setups that mount a volume at `/root/.vera/` still work. You can also set `VERA_HOME` to choose the data path.

## Run other commands

Pass a command after the image name to replace `mcp`:

```bash
docker run --rm -v "$(pwd):/workspace" -w /workspace ghcr.io/veratools/vera:cpu index .
docker run --rm -v "$(pwd):/workspace" -w /workspace ghcr.io/veratools/vera:cpu search "authentication logic"
docker run --rm -v "$(pwd):/workspace" -w /workspace ghcr.io/veratools/vera:cpu stats
```

## GPU requirements

| Image | Host requirements |
|---|---|
| `cuda` | NVIDIA driver that supports CUDA 13.1, and the [NVIDIA Container Toolkit](https://docs.nvidia.com/datacenter/cloud-native/container-toolkit/latest/install-guide.html) |
| `rocm` | AMD GPU with ROCm 6 drivers; `/dev/kfd` and `/dev/dri` accessible |
| `openvino` | Intel GPU or iGPU with the Intel compute runtime; `/dev/dri` accessible; Linux x86_64 only |

## Build the images yourself

The Dockerfiles package a prebuilt binary, so build Vera first. It must be a Linux x86_64 build. From the repository root on a Linux x86_64 machine:

```bash
./scripts/bootstrap-vendored-grammars.sh
cargo build --locked --release -p vera-cli
mkdir -p dist && cp target/release/vera dist/

docker build -f docker/Dockerfile.cpu -t vera:cpu .
docker build -f docker/Dockerfile.cuda -t vera:cuda .
docker build -f docker/Dockerfile.rocm -t vera:rocm .
docker build -f docker/Dockerfile.openvino -t vera:openvino .
```

The bootstrap script downloads grammar sources that the build needs. On macOS or Windows, run the build inside a Linux container:

```bash
docker run --rm --platform linux/amd64 -v "$(pwd):/src" -w /src rust:1.88 \
  bash -c './scripts/bootstrap-vendored-grammars.sh && cargo build --locked --release -p vera-cli && mkdir -p dist && cp target/release/vera dist/'
```

The ROCm Dockerfile pulls its base from Vera's GitHub Container Registry mirror, which can require `docker login ghcr.io`.

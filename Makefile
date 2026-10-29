# zatara-netcoredbg development entry point.
#
# Every target runs the command the README names or the one
# .github/workflows/release.yml runs. This is not a substitute for CI: the
# release workflow builds the proxy on five runners, and this file can only
# build for the machine it runs on.
#
# GNU make only (macOS ships 3.81, so no 4.x constructs). The knobs are
# variables: make check CARGO=cargo PYTHON=python3 VERSION=0.2.0

SHELL := /bin/bash
CARGO ?= cargo
PYTHON ?= python3

WASM_TARGET := wasm32-wasip2
WASM_ARTIFACT := target/$(WASM_TARGET)/release/zatara.wasm
EXTENSION_WASM := extension.wasm

# extension.toml is the version Zed shows and the name the release assets carry.
VERSION ?= $(shell sed -n 's/^version = "\([^"]*\)"/\1/p' extension.toml | head -1)
NETCOREDBG_VERSION ?= 3.2.0-1092
NETCOREDBG_REPO ?= Samsung/netcoredbg
DIST ?= dist

# The release asset tag for this machine, mapped from the rustc host triple.
# An unmapped triple is left alone so a new platform fails loudly in
# stage-proxy instead of naming an asset that never shipped.
RUST_HOST := $(shell $(CARGO) -vV 2>/dev/null | sed -n 's/^host: //p')
HOST_TARGET := $(shell echo $(RUST_HOST) | sed \
	-e 's/aarch64-apple-darwin/macos-arm64/' \
	-e 's/x86_64-apple-darwin/macos-x64/' \
	-e 's/aarch64-.*linux/linux-arm64/' \
	-e 's/x86_64-.*linux/linux-x64/' \
	-e 's/.*-w64-mingw.*/win-x64/' \
	-e 's/.*-pc-windows-msvc/win-x64/')
BIN_EXT := $(if $(filter win-x64,$(HOST_TARGET)),.exe,)
PROXY_ASSET := zatara-proxy-$(VERSION)-$(HOST_TARGET)$(BIN_EXT)

# Zed's per-extension work dir: where it caches the binaries it downloads and
# the only place the proxy may write diagnostics. Zed's data directory is
# %LOCALAPPDATA%\Zed on Windows, ~/Library/Application Support/Zed on macOS,
# and ~/.local/share/Zed on Linux; override ZED_WORK_DIR if a checkout
# disagrees.
UNAME_S := $(shell uname -s)
ifneq (,$(findstring MINGW,$(UNAME_S)))
ZED_WORK_DIR ?= $(LOCALAPPDATA)/Zed/extensions/work/zatara-netcoredbg
else ifneq (,$(findstring MSYS,$(UNAME_S)))
ZED_WORK_DIR ?= $(LOCALAPPDATA)/Zed/extensions/work/zatara-netcoredbg
else ifeq ($(UNAME_S),Darwin)
ZED_WORK_DIR ?= $(HOME)/Library/Application Support/Zed/extensions/work/zatara-netcoredbg
else
ZED_WORK_DIR ?= $(HOME)/.local/share/Zed/extensions/work/zatara-netcoredbg
endif
CACHE_DIR := $(ZED_WORK_DIR)/binary-cache

PROSE := README.md docs/templates/*.md examples/program-debug/README.md

.PHONY: help doctor build wasm test check clippy fmt-check voice \
	stage-proxy stage-netcoredbg manifest verify-assets release-check clean distclean \
	fix-perms dev-update

help: ## List the targets
	@awk -F ':.*## ' '/^[a-z][a-z-]*:.*## / { printf "  %-18s %s\n", $$1, $$2 }' $(MAKEFILE_LIST)

doctor: ## Report the toolchain and the binary cache a session depends on
	@echo "cargo:      `$(CARGO) --version 2>/dev/null || echo MISSING`"
	@echo "python:     `$(PYTHON) --version 2>/dev/null || echo MISSING`"
	@if rustup target list --installed 2>/dev/null | grep -qx "$(WASM_TARGET)"; then \
	  echo "wasm target: $(WASM_TARGET) installed"; \
	else \
	  echo "wasm target: $(WASM_TARGET) MISSING. Run: rustup target add $(WASM_TARGET)"; \
	fi
	@echo "host asset: $(PROXY_ASSET) (triple $(RUST_HOST))"
	@if [ -d "$(CACHE_DIR)" ]; then \
	  echo "cache:      $(CACHE_DIR)"; \
	  for f in "$(CACHE_DIR)"/*; do \
	    [ -f "$$f" ] || continue; \
	    case "$$f" in *.zip|*.part) continue ;; esac; \
	    [ -x "$$f" ] || echo "  NOT EXECUTABLE: $$f (chmod +x it, or a Zed spawn fails: os error 13)"; \
	  done; \
	else \
	  echo "cache:      none yet (the first session creates $(CACHE_DIR))"; \
	fi

build: ## cargo build --workspace: extension lib, proxy, and the run-case helper
	@$(CARGO) build --workspace

wasm: ## Build the extension wasm and install it as extension.wasm
	@$(CARGO) build --release --target $(WASM_TARGET) -p zatara
	@cp $(WASM_ARTIFACT) $(EXTENSION_WASM)
	@echo "installed $(EXTENSION_WASM) (`wc -c < $(EXTENSION_WASM) | tr -d ' '` bytes)"
	@echo "restart Zed to load it: a dev extension reads its wasm at startup, not on change"

# The tree-gate suite is green on Apple-silicon macOS: parent_of reads
# /proc on Linux, proc_pidinfo on macOS, and the Toolhelp map on Windows.
test: ## cargo test --workspace
	@$(CARGO) test --workspace

check: ## The local gate: workspace build, extension wasm, then the suites
	@$(MAKE) --no-print-directory build
	@$(MAKE) --no-print-directory wasm
	@$(MAKE) --no-print-directory test

# Advisory, not a gate: the tree carries lint warnings today (long test-fn names
# above all), so these report without failing the build.
clippy: ## cargo clippy over the workspace and its tests (advisory)
	@$(CARGO) clippy --workspace --all-targets

# Advisory: the tree is not rustfmt-clean, so `cargo fmt` here would rewrite most
# of it. Do not wire this into check until the tree is formatted once and for all.
fmt-check: ## Report rustfmt drift without touching files (advisory)
	@$(CARGO) fmt --all -- --check

stage-proxy: ## Build the proxy in release and stage it under its release asset name
	@case "$(HOST_TARGET)" in \
	  win-x64|linux-x64|linux-arm64|macos-x64|macos-arm64) ;; \
	  *) echo "no release asset ships for HOST_TARGET=$(HOST_TARGET) (detected triple $(RUST_HOST)). Pass one of: win-x64 linux-x64 linux-arm64 macos-x64 macos-arm64"; exit 1 ;; \
	esac
	@$(CARGO) build -p zatara-proxy --release
	@mkdir -p $(DIST)
	@cp target/release/zatara-proxy$(BIN_EXT) $(DIST)/$(PROXY_ASSET)
	@echo "staged $(DIST)/$(PROXY_ASSET)"

# Same naming the release workflow uses, archive extension included, so the
# manifest it builds hashes bytes that match what staging produced here.
stage-netcoredbg: ## Fetch the pinned netcoredbg asset for this host (needs gh)
	@case "$(HOST_TARGET)" in \
	  macos-x64) echo "no netcoredbg asset exists for macos-x64: upstream ships no Intel macOS build"; exit 0 ;; \
	  win-x64) pattern=netcoredbg-win64.zip ;; \
	  linux-x64) pattern=netcoredbg-linux-amd64.tar.gz ;; \
	  linux-arm64) pattern=netcoredbg-linux-arm64.tar.gz ;; \
	  macos-arm64) pattern=netcoredbg-osx-arm64.zip ;; \
	  *) echo "no netcoredbg asset name for HOST_TARGET=$(HOST_TARGET) (detected triple $(RUST_HOST)). Pass one of: win-x64 linux-x64 linux-arm64 macos-arm64"; exit 1 ;; \
	esac; \
	ext="$${pattern##*.}"; \
	mkdir -p $(DIST) ndbg-raw; \
	gh release download $(NETCOREDBG_VERSION) --repo $(NETCOREDBG_REPO) --pattern "$$pattern" --dir ndbg-raw; \
	cp "ndbg-raw/$$pattern" "$(DIST)/netcoredbg-$(NETCOREDBG_VERSION)-$(HOST_TARGET).$$ext"; \
	echo "staged $(DIST)/netcoredbg-$(NETCOREDBG_VERSION)-$(HOST_TARGET).$$ext"

manifest: ## Hash every staged asset into the version-pinned manifest.json
	@$(PYTHON) scripts/build_release_manifest.py --assets-dir $(DIST) --version $(VERSION)

verify-assets: ## Refuse any staged asset whose sha256 differs from manifest.json
	@$(PYTHON) scripts/verify_release_assets.py --manifest $(DIST)/manifest.json --assets-dir $(DIST)

release-check: ## Rehearse the release steps this machine can run: stage, hash, verify
	@$(MAKE) --no-print-directory stage-proxy
	@$(MAKE) --no-print-directory manifest
	@$(MAKE) --no-print-directory verify-assets

clean: ## Drop build output, keeping the wasm Zed currently loads
	@$(CARGO) clean
	@rm -rf ndbg-raw

distclean: clean ## Also drop staged assets and the installed extension wasm
	@rm -rf $(DIST)
	@rm -f $(EXTENSION_WASM)
	@echo "removed $(EXTENSION_WASM): Zed cannot start a session until make wasm rebuilds it"

fix-perms: ## Ensure cached binaries are executable on Unix
	@if [ -d "$(CACHE_DIR)" ]; then \
	  find "$(CACHE_DIR)" -type f ! -name "*.zip" ! -name "*.part" -exec chmod +x {} +; \
	  echo "fixed permissions in $(CACHE_DIR)"; \
	else \
	  echo "cache dir $(CACHE_DIR) does not exist yet"; \
	fi

dev-update: ## Build wasm and remind to reinstall dev extension
	@$(MAKE) --no-print-directory wasm
	@echo ""
	@echo "Wasm rebuilt. Reinstall the dev extension in Zed:"
	@echo "  zed: install dev extension"
	@echo "Zed loads the wasm at startup; a restart is required after reinstall."

# Everything runs inside podman: nothing is installed or executed on the host.
DEV_IMAGE ?= localhost/pdfmatrix-dev
# Single-architecture images, so that emulated test runs never re-point the multi-arch node tag.
NODE_AMD64 ?= docker.io/amd64/node:22-bookworm-slim
NODE_ARM64 ?= docker.io/arm64v8/node:22-bookworm-slim
VOLUMES    = -v pdfnet-node-modules:/app/node_modules -v pdfnet-cargo-registry:/usr/local/cargo/registry -v pdfnet-target:/target
RUN        = podman run --rm -v $(CURDIR):/app:Z $(VOLUMES) -w /app $(DEV_IMAGE)

# Office/HTML conversion service (separate container, reached over a podman network).
GOTENBERG_IMG ?= docker.io/gotenberg/gotenberg:8.37.0
NETWORK       ?= pdfnet
GOTENBERG     ?= pdfnet-gotenberg
RUN_NET        = podman run --rm --network $(NETWORK) -e GOTENBERG_URL=http://$(GOTENBERG):3000 -v $(CURDIR):/app:Z $(VOLUMES) -w /app $(DEV_IMAGE)
TEST_ON    = podman run --rm -v $(CURDIR):/app:Z -w /app --platform

.PHONY: dev-image install build build-amd64 build-arm64 build-all test test-amd64 test-arm64 examples shell image clean gotenberg gotenberg-stop test-all test-musl

dev-image:
	podman build --platform linux/$$(podman info --format "{{.Host.Arch}}") --target dev -t $(DEV_IMAGE) -f Containerfile .

install:
	$(RUN) npm install

# Native addon for the host architecture of the podman VM + TypeScript
build:
	$(RUN) npm run build

# Cross-compiled addons (napi-rs cross toolchains), usable from any host architecture
build-amd64:
	$(RUN) sh -c 'npm run build:native:x64 && npm run build:ts'

build-arm64:
	$(RUN) sh -c 'npm run build:native:arm64 && npm run build:ts'

build-all:
	$(RUN) npm run build:all

test:
	$(RUN) npm test

# Run the suite against one architecture's binary (emulated when it is not the VM's own)
test-amd64:
	$(TEST_ON) linux/amd64 $(NODE_AMD64) node --test "test/**/*.test.mjs"

test-arm64:
	$(TEST_ON) linux/arm64 $(NODE_ARM64) node --test "test/**/*.test.mjs"

examples: build
	$(RUN) node examples/run-all.mjs

shell:
	podman run --rm -it -v $(CURDIR):/app:Z $(VOLUMES) -w /app $(DEV_IMAGE) bash

# Self-contained image that builds and runs the test-suite (what CI runs)
image:
	podman build --target test -t localhost/pdfmatrix -f Containerfile .

clean:
	rm -rf dist out native/*.node
	podman volume rm -f pdfnet-node-modules pdfnet-target

# Starts Gotenberg (LibreOffice + Chromium) and waits until it is healthy.
gotenberg:
	podman network exists $(NETWORK) || podman network create $(NETWORK)
	podman container exists $(GOTENBERG) || podman run -d --name $(GOTENBERG) --network $(NETWORK) -p 3000:3000 $(GOTENBERG_IMG)
	podman run --rm --network $(NETWORK) $(DEV_IMAGE) node -e 'const u="http://$(GOTENBERG):3000/health";(async()=>{for(let i=0;i<60;i++){try{if((await fetch(u)).ok){console.log("gotenberg ready");return}}catch{}await new Promise(r=>setTimeout(r,1000))}console.error("gotenberg not ready");process.exit(1)})()'

gotenberg-stop:
	podman rm -f $(GOTENBERG)

# Full suite including Office/HTML conversion tests.
test-all: gotenberg
	$(RUN_NET) sh -c 'npm run build:ts && npm test'

# Alpine / musl: builds and tests the linux-<arch>-musl addon in its own image
# (for the VM's architecture; CI builds both x64 and arm64 on native runners).
test-musl:
	podman build --target test-musl -t localhost/pdfmatrix-musl-test -f Containerfile .
	podman run --rm localhost/pdfmatrix-musl-test npm test

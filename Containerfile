# Dev/build image: Rust toolchain + Node 22. Everything (cargo, npm, tests) runs in here.
FROM docker.io/library/node:22-bookworm-slim AS node

FROM docker.io/library/rust:1-slim-bookworm AS dev
COPY --from=node /usr/local/bin/node /usr/local/bin/node
COPY --from=node /usr/local/lib/node_modules /usr/local/lib/node_modules
RUN ln -s /usr/local/lib/node_modules/npm/bin/npm-cli.js /usr/local/bin/npm \
 && ln -s /usr/local/lib/node_modules/npm/bin/npx-cli.js /usr/local/bin/npx \
 && rustup component add clippy rustfmt \
 && rustup target add x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu
ENV CARGO_TARGET_DIR=/target
WORKDIR /app
CMD ["bash"]

# Self-contained CI image: builds the native addon and runs the test-suite.
FROM dev AS test
COPY . .
RUN npm ci && npm run build
CMD ["npm", "test"]

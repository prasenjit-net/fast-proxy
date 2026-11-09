.PHONY: build ui dev-server dev-ui run clean

## Build the single release binary (frontend + backend).
build: ui
	cargo build --release
	@echo "→ single binary at target/release/rusty-template"

## Build the frontend into ui/dist (embedded by the Rust build).
ui:
	cd ui && npm install && npm run build

## Run the backend (serves ui/dist if it has been built).
dev-server:
	cargo run

## Run the Vite dev server with hot reload (proxies /api and /ws to :8080).
dev-ui:
	cd ui && npm run dev

## Build everything, then run the release binary.
run: build
	./target/release/rusty-template

clean:
	cargo clean
	rm -rf ui/dist ui/node_modules

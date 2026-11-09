#!/bin/bash
# Runner-side half of the OpenWrt SDK build: fetch the SDK, hand directories to
# the container user and run sdk.sh in ghcr.io/openwrt/sdk.

set -euo pipefail

ACTION_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

output() { echo "$1=$2" >> "$GITHUB_OUTPUT"; }
export_env() { echo "$1=$2" >> "$GITHUB_ENV"; export "$1=$2"; }

image() {
	local image="ghcr.io/openwrt/sdk:${ARCH:?ARCH is required}"
	local attempt
	for attempt in 1 2 3; do
		docker pull -q "$image" && break
		[ "$attempt" -lt 3 ] || exit 1
		sleep 20
	done

	local sdk_dir="$RUNNER_TEMP/openwrt-sdk"
	local keys_dir="$RUNNER_TEMP/openwrt-keys"
	rm -rf "$sdk_dir" "$keys_dir"
	mkdir -p "$sdk_dir" "$keys_dir" "$RUNNER_TEMP/openwrt-sdk-archive" "$RUNNER_TEMP/openwrt-rust-host"
	# The container user writes the rust/host archive here.
	chmod 0777 "$RUNNER_TEMP/openwrt-rust-host"

	local cid
	cid=$(docker create "$image")
	docker cp "$cid:/builder/." "$sdk_dir"
	docker rm "$cid" > /dev/null

	# Release images unpack the SDK at build time; snapshot images only carry
	# setup.sh and the signing keys and download the SDK on first start.
	local bundled=false
	if [ -f "$sdk_dir/rules.mk" ] && [ -d "$sdk_dir/staging_dir" ]; then
		bundled=true
	fi
	if [ -d "$sdk_dir/keys" ]; then
		find "$sdk_dir/keys" -name '*.asc' -exec mv -t "$keys_dir" {} +
	fi
	rm -rf "$sdk_dir/keys" "$sdk_dir/setup.sh"

	local env_dump
	env_dump=$(docker image inspect -f '{{range .Config.Env}}{{println .}}{{end}}' "$image")
	image_env() { sed -n "s/^$1=//p" <<< "$env_dump" | head -n1; }

	export_env SDK_IMAGE "$image"
	export_env SDK_DIR "$sdk_dir"
	export_env SDK_KEYS_DIR "$keys_dir"
	export_env SDK_ARCHIVE_DIR "$RUNNER_TEMP/openwrt-sdk-archive"
	export_env RUST_CACHE_DIR "$RUNNER_TEMP/openwrt-rust-host"
	export_env SDK_UID "$(docker run --rm --entrypoint id "$image" -u)"
	export_env SDK_GID "$(docker run --rm --entrypoint id "$image" -g)"
	export_env SDK_TARGET "$(image_env TARGET)"
	export_env SDK_VERSION_PATH "$(image_env VERSION_PATH)"
	export_env SDK_DOWNLOAD_FILE "$(image_env DOWNLOAD_FILE)"
	output bundled "$bundled"
	output arch "$ARCH"

	echo "Image: $image (bundled SDK: $bundled, target ${SDK_TARGET:-?}, ${SDK_VERSION_PATH:-?})"
}

# Download and verify the signed sha256sums of the SDK's target, trying each
# upstream in turn. A snapshot can be re-signed while it is being fetched, so
# a bad signature or a missing entry is retried after a pause.
resolve_sums() {
	local gnupg round host base file count
	gnupg=$(mktemp -d)
	gpg --homedir "$gnupg" --batch --quiet --import "$SDK_KEYS_DIR"/*.asc

	for round in 1 2 3; do
		for host in $UPSTREAM_URLS; do
			base="${host%/}/$SDK_VERSION_PATH/targets/$SDK_TARGET"
			echo "Fetching $base/sha256sums"
			if ! curl -fsSL --connect-timeout 20 --max-time 300 --retry 5 --retry-all-errors --retry-delay 5 \
				-o "$RUNNER_TEMP/sha256sums" "$base/sha256sums" ||
				! curl -fsSL --connect-timeout 20 --max-time 120 --retry 5 --retry-all-errors --retry-delay 5 \
				-o "$RUNNER_TEMP/sha256sums.asc" "$base/sha256sums.asc"; then
				echo "::warning::Cannot fetch sha256sums from $host"
				continue
			fi
			if ! gpg --homedir "$gnupg" --batch --verify "$RUNNER_TEMP/sha256sums.asc" "$RUNNER_TEMP/sha256sums"; then
				echo "::warning::sha256sums signature from $host did not verify"
				continue
			fi
			count=$(grep -c "$SDK_DOWNLOAD_FILE" "$RUNNER_TEMP/sha256sums" || true)
			if [ "$count" -lt 1 ]; then
				echo "::warning::No file matching '$SDK_DOWNLOAD_FILE' in sha256sums from $host"
				continue
			fi
			[ "$count" -eq 1 ] || echo "::warning::$count files match '$SDK_DOWNLOAD_FILE'; using the first"
			grep -m1 "$SDK_DOWNLOAD_FILE" "$RUNNER_TEMP/sha256sums" > "$RUNNER_TEMP/sdk.sha256"
			file=$(cut -d'*' -f2 "$RUNNER_TEMP/sdk.sha256")
			SDK_HOST="$host"
			SDK_URL="$base/$file"
			SDK_FILE="$file"
			SDK_SHA256=$(cut -d' ' -f1 "$RUNNER_TEMP/sdk.sha256")
			rm -rf "$gnupg"
			return 0
		done
		if [ "$round" -lt 3 ]; then sleep 60; fi
	done
	rm -rf "$gnupg"
	echo "::error::No upstream served a verifiable sha256sums for $SDK_VERSION_PATH/targets/$SDK_TARGET"
	return 1
}

resolve() {
	resolve_sums
	echo "SDK: $SDK_FILE ($SDK_SHA256) from $SDK_HOST"
	output file "$SDK_FILE"
	output sha256 "$SDK_SHA256"
}

archive_ok() {
	[ -f "$SDK_ARCHIVE_DIR/$SDK_FILE" ] &&
		(cd "$SDK_ARCHIVE_DIR" && echo "$SDK_SHA256 *$SDK_FILE" | sha256sum -c --status -)
}

fetch() {
	resolve_sums
	local downloaded=false attempt host url
	if archive_ok; then
		echo "Using cached $SDK_FILE"
	else
		downloaded=true
		for attempt in 1 2 3; do
			find "$SDK_ARCHIVE_DIR" -mindepth 1 -not -name "$SDK_FILE" -delete
			for host in $SDK_HOST $UPSTREAM_URLS; do
				url="${host%/}/$SDK_VERSION_PATH/targets/$SDK_TARGET/$SDK_FILE"
				echo "Downloading $url"
				# Give up on a stalled mirror (under 256 KiB/s for two minutes)
				# and resume from the next one.
				if curl -fL --connect-timeout 20 --retry 5 --retry-all-errors --retry-delay 10 \
					--speed-limit 262144 --speed-time 120 -C - \
					-o "$SDK_ARCHIVE_DIR/$SDK_FILE" "$url"; then
					break
				fi
				echo "::warning::Download from $host failed"
			done
			if archive_ok; then
				break
			fi
			# A snapshot published during the download changes the archive;
			# fetch the new signed checksums and start over once more.
			echo "::warning::$SDK_FILE does not match its signed checksum (attempt $attempt)"
			rm -f "$SDK_ARCHIVE_DIR/$SDK_FILE"
			[ "$attempt" -lt 3 ] || exit 1
			resolve_sums
		done
	fi

	echo "$SDK_SHA256 *$SDK_FILE verified"
	output sha256 "$SDK_SHA256"
	output downloaded "$downloaded"
	tar -xf "$SDK_ARCHIVE_DIR/$SDK_FILE" --strip-components=1 --no-same-owner -C "$SDK_DIR"
}

run() {
	docker run --rm \
		--env BUILD_LOG \
		--env IGNORE_ERRORS \
		--env NO_REFRESH_CHECK \
		--env NO_SHFMT_CHECK \
		--env PACKAGES \
		--env V \
		-v "$SDK_DIR:/builder" \
		-v "$GITHUB_WORKSPACE:/feed" \
		-v "$RUST_CACHE_DIR:/cache" \
		-v "$ACTION_DIR/sdk.sh:/ci/sdk.sh:ro" \
		"$SDK_IMAGE" bash /ci/sdk.sh "$@"
}

prepare() {
	sudo chown -R "$SDK_UID:$SDK_GID" "$SDK_DIR" "$GITHUB_WORKSPACE"
	run prepare
	local key=""
	[ ! -f "$SDK_DIR/.ci-rust-key" ] || key=$(cat "$SDK_DIR/.ci-rust-key")
	echo "rust/host cache id: ${key:-none}"
	output rust-key "$key"
}

rust() {
	if [ "${RUST_CACHE_HIT:-}" = true ] && [ -f "$RUST_CACHE_DIR/rust-host.tar" ]; then
		local stamp
		stamp=$(date +%s)
		sudo tar -xpf "$RUST_CACHE_DIR/rust-host.tar" -C "$SDK_DIR"
		# Fresh feeds are newer than the cached stamps; OpenWrt would rebuild
		# from that alone. Stamp names still carry the package content hash.
		tar -tf "$RUST_CACHE_DIR/rust-host.tar" | grep -E '(^|/)\.[^/]+$' |
			(cd "$SDK_DIR" && sudo xargs -r -d '\n' touch -c -d "@$stamp")
		sudo chown -R "$SDK_UID:$SDK_GID" "$SDK_DIR"
		run rust warm
	else
		rm -f "$RUST_CACHE_DIR/rust-host.tar"
		run rust cold
	fi
}

finish() {
	if [ -n "${SDK_DIR:-}" ]; then
		[ ! -d "$SDK_DIR/bin" ] || sudo mv "$SDK_DIR/bin" "$GITHUB_WORKSPACE/"
		[ ! -d "$SDK_DIR/logs" ] || sudo mv "$SDK_DIR/logs" "$GITHUB_WORKSPACE/"
	fi
	sudo chown -R "$(id -u):$(id -g)" "$GITHUB_WORKSPACE"
}

"$@"

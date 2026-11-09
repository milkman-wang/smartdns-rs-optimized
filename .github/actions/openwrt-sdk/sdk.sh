#!/bin/bash
# Runs inside ghcr.io/openwrt/sdk with the SDK at /builder and this repository
# at /feed. The build steps follow openwrt/gh-action-sdk's entrypoint for a
# PACKAGES build; rust/host is split out so the workflow can cache it.

set -eo pipefail

cd /builder

FEEDNAME=action
BUILD_LOG="${BUILD_LOG:-1}"
RUST_DIR=feeds/packages/lang/rust
RUST_TARGET=package/feeds/packages/rust/host/compile

group() { echo "::group::  $*"; }
endgroup() { echo "::endgroup::"; }

prepare() {
	sed \
		-e 's,https://git.openwrt.org/feed/,https://github.com/openwrt/,' \
		-e 's,https://git.openwrt.org/openwrt/,https://github.com/openwrt/,' \
		-e 's,https://git.openwrt.org/project/,https://github.com/openwrt/,' \
		feeds.conf.default > feeds.conf
	echo "src-link $FEEDNAME /feed/" >> feeds.conf

	group "feeds.conf"
	cat feeds.conf
	endgroup

	group "feeds update -a"
	./scripts/feeds update -a
	endgroup

	group "make defconfig"
	make defconfig
	endgroup

	local badhash_msg="HASH does not match |HASH uses deprecated hash,|HASH is missing,"
	local pkg ret patches_dir files_dir
	for pkg in $PACKAGES; do
		group "feeds install -p $FEEDNAME -f $pkg"
		./scripts/feeds install -p "$FEEDNAME" -f "$pkg"
		endgroup

		group "make package/$pkg/download"
		make BUILD_LOG="$BUILD_LOG" IGNORE_ERRORS="$IGNORE_ERRORS" "package/$pkg/download" V=s
		endgroup

		group "make package/$pkg/check"
		ret=0
		make BUILD_LOG="$BUILD_LOG" IGNORE_ERRORS="$IGNORE_ERRORS" "package/$pkg/check" V=s 2>&1 | tee logtmp || ret=$?
		endgroup
		if [ "$ret" -ne 0 ]; then
			echo 'Package check failed'
			exit "$ret"
		fi
		if grep -qE "$badhash_msg" logtmp; then
			echo "Package HASH check failed"
			exit 1
		fi

		patches_dir=$(find /feed -path "*/$pkg/patches")
		if [ -d "$patches_dir" ] && [ -z "$NO_REFRESH_CHECK" ]; then
			group "make package/$pkg/refresh"
			make BUILD_LOG="$BUILD_LOG" IGNORE_ERRORS="$IGNORE_ERRORS" "package/$pkg/refresh" V=s
			endgroup
			if ! git -C "$patches_dir" diff --quiet -- .; then
				echo "Dirty patches detected, please refresh and review the diff"
				git -C "$patches_dir" checkout -- .
				exit 1
			fi
			group "make package/$pkg/clean"
			make BUILD_LOG="$BUILD_LOG" IGNORE_ERRORS="$IGNORE_ERRORS" "package/$pkg/clean" V=s
			endgroup
		fi

		files_dir=$(find /feed -path "*/$pkg/files")
		if [ -d "$files_dir" ] && [ -z "$NO_SHFMT_CHECK" ]; then
			find "$files_dir" -name "*.init" -exec shfmt -w -sr -s '{}' \;
			if ! git -C "$files_dir" diff --quiet -- .; then
				echo "init script must be formatted. Please run through shfmt -w -sr -s"
				git -C "$files_dir" checkout -- .
				exit 1
			fi
		fi
	done

	# Cache identity for rust/host: the SDK toolchain plus everything OpenWrt
	# hashes into the rust/host stamps (the package files and stamp rules).
	rm -f .ci-rust-key
	if [ -L package/feeds/packages/rust ] && [ -f "$RUST_DIR/Makefile" ]; then
		local version toolchain digest
		version=$(sed -n 's/^PKG_VERSION:=//p' "$RUST_DIR/Makefile")
		toolchain=$(basename "$(find staging_dir -maxdepth 1 -name 'toolchain-*' -print -quit)")
		digest=$(find "$RUST_DIR" include/host-build.mk include/depends.mk -type f -not -path '*/.*' -print0 |
			LC_ALL=C sort -z | xargs -0 sha256sum | sha256sum | cut -c1-16)
		echo "rust-${version}-${toolchain}-${digest}" > .ci-rust-key
	fi
}

tree_files() {
	find staging_dir build_dir \( -type f -o -type l \) -print | LC_ALL=C sort
}

stamp_times() {
	{
		find build_dir/*/host/rustc-* staging_dir/hostpkg/stamp -maxdepth 1 -name '.*' -type f \
			-printf '%p %T@\n' 2>/dev/null || true
	} | LC_ALL=C sort
}

rust() {
	local mode="$1"
	if [ "$mode" = cold ]; then
		tree_files > /tmp/rust-before
		touch /tmp/rust-marker
		sleep 1
	else
		stamp_times > /tmp/rust-stamps-before
	fi

	group "make $RUST_TARGET ($mode cache)"
	make \
		BUILD_LOG="$BUILD_LOG" \
		IGNORE_ERRORS="$IGNORE_ERRORS" \
		CONFIG_AUTOREMOVE=y \
		V="$V" \
		-j "$(nproc)" \
		"$RUST_TARGET"
	endgroup

	if [ "$mode" = cold ]; then
		# Everything the rust/host build added or changed, including the
		# zero-size stamps CONFIG_AUTOREMOVE keeps in its build directory.
		tree_files > /tmp/rust-after
		{
			LC_ALL=C comm -13 /tmp/rust-before /tmp/rust-after
			find staging_dir build_dir -newer /tmp/rust-marker \( -type f -o -type l \) -print
		} | LC_ALL=C sort -u > /tmp/rust-files
		grep -q '/stamp/\.rust_installed$' /tmp/rust-files
		echo "Caching $(wc -l < /tmp/rust-files) rust/host files"
		tar -cf /cache/rust-host.tar -T /tmp/rust-files
		ls -l /cache/rust-host.tar
	else
		stamp_times > /tmp/rust-stamps-after
		if ! cmp -s /tmp/rust-stamps-before /tmp/rust-stamps-after; then
			echo "::warning::rust/host was rebuilt even though its cache was restored"
			diff /tmp/rust-stamps-before /tmp/rust-stamps-after || true
		else
			echo "rust/host stamps unchanged: cached build reused"
		fi
	fi
}

packages() {
	make \
		-f .config \
		-f tmp/.packagedeps \
		-f <(echo "\$(info \$(sort \$(package-y) \$(package-m)))"; echo -en "a:\n\t@:") \
		| tr ' ' '\n' > enabled-package-subdirs.txt

	local pkg ret=0
	for pkg in $PACKAGES; do
		if ! grep -m1 -qE "(^|/)$pkg$" enabled-package-subdirs.txt; then
			echo "::warning file=$pkg::Skipping $pkg due to unsupported architecture"
			continue
		fi

		make \
			BUILD_LOG="$BUILD_LOG" \
			IGNORE_ERRORS="$IGNORE_ERRORS" \
			CONFIG_AUTOREMOVE=y \
			V="$V" \
			-j "$(nproc)" \
			"package/$pkg/compile" || {
				ret=$?
				break
			}
	done
	exit "$ret"
}

case "$1" in
	prepare) prepare ;;
	rust) rust "$2" ;;
	packages) packages ;;
	*) echo "usage: $0 prepare|rust cold|rust warm|packages" >&2; exit 2 ;;
esac

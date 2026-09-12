# SPDX-License-Identifier: GPL-3.0-only

include $(TOPDIR)/rules.mk

PKG_NAME:=smartdns-rs
PKG_VERSION:=0.13.1
PKG_RELEASE:=1

PKG_SOURCE_VERSION:=8b329415972b76d8ab5471417254120ef8fe6e72
PKG_SOURCE_PROJECT:=smartdns-rs-optimized
PKG_SOURCE:=$(PKG_NAME)-$(PKG_SOURCE_VERSION).tar.gz
PKG_SOURCE_URL:=https://codeload.github.com/milkman-wang/smartdns-rs-optimized/tar.gz/$(PKG_SOURCE_VERSION)?
PKG_HASH:=a2629f5d3a3a093035485d24afda5c37556a3423bfc7c6a02b1a2fe6b34cffd7
PKG_BUILD_DIR:=$(BUILD_DIR)/$(PKG_NAME)$(if $(BUILD_VARIANT),-$(BUILD_VARIANT))/$(PKG_SOURCE_PROJECT)-$(PKG_SOURCE_VERSION)

PKG_MAINTAINER:=SmartDNS-rs contributors
PKG_LICENSE:=GPL-3.0-only
PKG_LICENSE_FILES:=LICENSE
PKG_BUILD_DEPENDS:=rust/host
PKG_BUILD_PARALLEL:=1

# Avoid desktop-only service management, the resolver CLI, and self-update.
# The protocol and nftables features below are the OpenWrt runtime surface.
RUST_PKG_FEATURES:=dns-over-tls,dns-over-https,dns-over-quic,dns-over-h3,mdns,dnssec,nft,nom-recipes-all,http_client,serde
ifeq ($(BUILD_VARIANT),webui)
  RUST_PKG_FEATURES+=webui
endif

include $(INCLUDE_DIR)/package.mk

include $(TOPDIR)/feeds/packages/lang/rust/rust-package.mk

# OpenWrt's pentium4 target is compiled through Rust's i586 target, whose
# conservative defaults omit SSE/SSE2. Test the resolved Rust target after
# rust-package.mk has loaded rust-values.mk; ARCH is not available early enough
# in every supported SDK generation.
ifeq ($(RUSTC_TARGET_ARCH),i586-unknown-linux-musl)
  CARGO_RUSTFLAGS+=-Ctarget-feature=+sse,+sse2
endif

# rust-values.mk overrides Cargo.toml with opt-level=z. Put this assignment
# after the SDK variables so release packages retain the measured speed gain.
CARGO_PKG_VARS+=CARGO_PROFILE_RELEASE_OPT_LEVEL=3
CARGO_PKG_ARGS+=--no-default-features

define Package/smartdns-rs
  SECTION:=net
  CATEGORY:=Network
  SUBMENU:=IP Addresses and Names
  TITLE:=SmartDNS-rs DNS server (headless)
  VARIANT:=headless
  DEFAULT_VARIANT:=1
  URL:=https://github.com/milkman-wang/smartdns-rs-optimized
  PROVIDES:=smartdns
  CONFLICTS:=smartdns
  DEPENDS:=$(RUST_ARCH_DEPENDS) +ca-bundle
endef

define Package/smartdns-rs-webui
  $(call Package/smartdns-rs)
  TITLE:=SmartDNS-rs DNS server with Rust WebUI
  VARIANT:=webui
  DEFAULT_VARIANT:=
  PROVIDES:=smartdns smartdns-rs
  # Declare the variant conflict once; two directions create a Kconfig cycle.
  CONFLICTS:=smartdns smartdns-rs
endef

define Package/smartdns-rs-webui/description
SmartDNS-rs with its optional Rust management UI. The DNS engine and WebUI are
compiled into one executable; no C plugin or separate web server is required.
endef

define Package/smartdns-rs/description
SmartDNS-rs is an asynchronous local DNS server that queries multiple upstream
resolvers and returns the fastest usable result. This package includes procd,
UCI, dnsmasq and LuCI-compatible integration for OpenWrt.
endef

define Package/smartdns-rs/conffiles
/etc/config/smartdns
/etc/smartdns/address.conf
/etc/smartdns/blacklist-ip.conf
/etc/smartdns/custom.conf
/etc/smartdns/domain-block.list
/etc/smartdns/domain-forwarding.list
endef

define Package/smartdns-rs-webui/conffiles
$(call Package/smartdns-rs/conffiles)
endef

define Package/smartdns-rs/install
	$(INSTALL_DIR) $(1)/usr/sbin
	$(INSTALL_BIN) $(PKG_INSTALL_DIR)/bin/smartdns $(1)/usr/sbin/smartdns

	$(INSTALL_DIR) $(1)/etc/init.d $(1)/etc/config $(1)/etc/uci-defaults
	$(INSTALL_BIN) ./files/etc/init.d/smartdns $(1)/etc/init.d/smartdns
	$(INSTALL_CONF) ./files/etc/config/smartdns $(1)/etc/config/smartdns
	$(INSTALL_BIN) ./files/etc/uci-defaults/90-smartdns-rs $(1)/etc/uci-defaults/90-smartdns-rs

	$(INSTALL_DIR) $(1)/etc/smartdns/conf.d $(1)/etc/smartdns/domain-set
	$(INSTALL_DIR) $(1)/etc/smartdns/ip-set $(1)/etc/smartdns/download
	$(INSTALL_CONF) ./files/etc/smartdns/address.conf $(1)/etc/smartdns/address.conf
	$(INSTALL_CONF) ./files/etc/smartdns/blacklist-ip.conf $(1)/etc/smartdns/blacklist-ip.conf
	$(INSTALL_CONF) ./files/etc/smartdns/custom.conf $(1)/etc/smartdns/custom.conf
	$(INSTALL_CONF) ./files/etc/smartdns/domain-block.list $(1)/etc/smartdns/domain-block.list
	$(INSTALL_CONF) ./files/etc/smartdns/domain-forwarding.list $(1)/etc/smartdns/domain-forwarding.list
endef

define Package/smartdns-rs-webui/install
	$(call Package/smartdns-rs/install,$(1))
	$(SED) "/option port '6053'/a\	option webui_enable '1'\n\	option webui_bind '0.0.0.0:6080'" $(1)/etc/config/smartdns
endef

$(eval $(call RustBinPackage,smartdns-rs))
$(eval $(call BuildPackage,smartdns-rs))
$(eval $(call RustBinPackage,smartdns-rs-webui))
$(eval $(call BuildPackage,smartdns-rs-webui))

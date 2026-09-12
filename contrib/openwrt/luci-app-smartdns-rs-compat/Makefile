# SPDX-License-Identifier: GPL-3.0-only

include $(TOPDIR)/rules.mk

PKG_NAME:=luci-app-smartdns-rs-compat
PKG_VERSION:=0.13.1
PKG_RELEASE:=1
PKG_LICENSE:=GPL-3.0-only
PKG_MAINTAINER:=SmartDNS-rs contributors

LUCI_TITLE:=Lua LuCI support for SmartDNS-rs
LUCI_DESCRIPTION:=Configure SmartDNS-rs using Lua CBI on legacy and modern LuCI
LUCI_DEPENDS:=+luci-base +!PACKAGE_smartdns-rs-webui:smartdns-rs +PACKAGE_smartdns-rs-webui:smartdns-rs-webui
# CBI is in luci-base on old firmware. On modern firmware install luci-compat.
# Ship source under root/ so luci.mk does not add luci-lua-runtime, which
# does not exist on legacy firmware, to the architecture-independent package.
LUCI_PKGARCH:=all

include $(TOPDIR)/feeds/luci/luci.mk

# call BuildPackage - OpenWrt buildroot signature

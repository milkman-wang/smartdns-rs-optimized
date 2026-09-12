# SPDX-License-Identifier: GPL-3.0-only

include $(TOPDIR)/rules.mk

PKG_NAME:=luci-app-smartdns-rs
PKG_VERSION:=0.13.1
PKG_RELEASE:=1
PKG_LICENSE:=GPL-3.0-only
PKG_MAINTAINER:=SmartDNS-rs contributors

LUCI_TITLE:=LuCI support for SmartDNS-rs
LUCI_DESCRIPTION:=Configure SmartDNS-rs with UCI and LuCI
LUCI_DEPENDS:=+luci-base +smartdns-rs
LUCI_PKGARCH:=all

include $(TOPDIR)/feeds/luci/luci.mk

# call BuildPackage - OpenWrt buildroot signature

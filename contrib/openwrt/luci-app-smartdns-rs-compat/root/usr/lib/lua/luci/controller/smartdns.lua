-- SPDX-License-Identifier: GPL-3.0-only
module("luci.controller.smartdns", package.seeall)

function index()
	if not nixio.fs.access("/etc/config/smartdns") then return end
	local page = entry({"admin", "services", "smartdns"},
		alias("admin", "services", "smartdns", "settings"), _("SmartDNS-rs"), 60)
	page.dependent = true
	page.i18n = "smartdns-rs-compat"
	page.acl_depends = {"luci-app-smartdns-rs"}
	-- Titled pages are shown as tabs.
	entry({"admin", "services", "smartdns", "settings"}, cbi("smartdns/smartdns"), _("Settings"), 10).leaf = true
	entry({"admin", "services", "smartdns", "upstream"}, cbi("smartdns/upstream"), _("Upstream DNS Servers"), 20).leaf = true
	entry({"admin", "services", "smartdns", "rules"}, cbi("smartdns/rules"), _("Rules"), 30).leaf = true
	entry({"admin", "services", "smartdns", "files"}, cbi("smartdns/files"), _("Download Files"), 40).leaf = true
	entry({"admin", "services", "smartdns", "log"}, cbi("smartdns/log"), _("Log"), 50).leaf = true
	entry({"admin", "services", "smartdns", "server"}, cbi("smartdns/server")).leaf = true
	entry({"admin", "services", "smartdns", "client"}, cbi("smartdns/client")).leaf = true
	entry({"admin", "services", "smartdns", "domain"}, cbi("smartdns/domain")).leaf = true
	entry({"admin", "services", "smartdns", "ip"}, cbi("smartdns/ip")).leaf = true
	entry({"admin", "services", "smartdns", "status"}, call("status")).leaf = true
end

function status()
	local service = require("luci.util").ubus("service", "list", {name = "smartdns"}) or {}
	local instances = service.smartdns and service.smartdns.instances or {}
	luci.http.prepare_content("application/json")
	luci.http.write_json({running = not not (instances.smartdns and instances.smartdns.running)})
end

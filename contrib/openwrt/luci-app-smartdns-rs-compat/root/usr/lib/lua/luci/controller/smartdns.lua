-- SPDX-License-Identifier: GPL-3.0-only
module("luci.controller.smartdns", package.seeall)

function index()
	if not nixio.fs.access("/etc/config/smartdns") then return end
	local page = entry({"admin", "services", "smartdns"},
		cbi("smartdns/smartdns"), _("SmartDNS-rs"), 60)
	page.dependent = true
	page.acl_depends = {"luci-app-smartdns-rs"}
	entry({"admin", "services", "smartdns", "log"}, cbi("smartdns/log")).leaf = true
	entry({"admin", "services", "smartdns", "server"}, cbi("smartdns/server")).leaf = true
	entry({"admin", "services", "smartdns", "status"}, call("status")).leaf = true
end

function status()
	local service = require("luci.util").ubus("service", "list", {name = "smartdns"}) or {}
	local instances = service.smartdns and service.smartdns.instances or {}
	luci.http.prepare_content("application/json")
	luci.http.write_json({running = not not (instances.smartdns and instances.smartdns.running)})
end

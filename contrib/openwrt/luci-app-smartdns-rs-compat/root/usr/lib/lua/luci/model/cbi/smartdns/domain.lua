-- SPDX-License-Identifier: GPL-3.0-only
local dispatcher = require "luci.dispatcher"
local http = require "luci.http"
local helpers = require "luci.model.smartdns_rs"
local m = Map("smartdns", translate("Domain Rule Lists"))
m.template = "smartdns/map"
m.redirect = dispatcher.build_url("admin", "services", "smartdns", "rules")

if not arg[1] or m.uci:get("smartdns", arg[1]) ~= "domain-rule-list" then
    http.redirect(m.redirect)
    return
end

local s = m:section(NamedSection, arg[1], "domain-rule-list")
s.anonymous = true
s.addremove = false
s:tab("general", translate("General Settings"))
s:tab("advanced", translate("Advanced Settings"))
local o

o = s:taboption("general", Flag, "enabled", translate("Enable"))
o.default = "1"
o.rmempty = false

o = s:taboption("general", Value, "name", translate("Name"))

o = s:taboption("general", Value, "domain_list_file", translate("Domain List File"))
o.rmempty = false
o.placeholder = "/etc/smartdns/domain-set/"

o = s:taboption("general", Value, "server_group", translate("Server Group"))
m.uci:foreach("smartdns", "server", function(server)
	if server.server_group then o:value(server.server_group) end
end)

o = s:taboption("general", ListValue, "block_domain_type", translate("Block Type"))
o:value("", translate("None"))
o:value("all", translate("IPv4 and IPv6"))
o:value("ipv4", translate("IPv4"))
o:value("ipv6", translate("IPv6"))

o = s:taboption("advanced", Value, "speed_check_mode", translate("Speed Check Mode"))
o:value("", translate("Default"))
o:value("ping,tcp:80,tcp:443")
o:value("ping,tcp:443,tcp:80")
o:value("tcp:80,tcp:443,ping")
o:value("tcp:443,tcp:80,ping")
o:value("http:80,https:443,ping")
o:value("none", translate("None"))
o.validate = helpers.validateSpeedModes

o = s:taboption("advanced", ListValue, "dualstack_ip_selection", translate("Dual-stack Selection"))
o:value("", translate("Default"))
o:value("yes", translate("Yes"))
o:value("no", translate("No"))

o = s:taboption("advanced", Flag, "force_aaaa_soa", translate("Force AAAA SOA"))

o = s:taboption("advanced", Value, "ipset_name", translate("Kernel IP Set"))
o.placeholder = "#4:route4,#6:route6"

o = s:taboption("advanced", Flag, "no_serve_expired", translate("Disable Stale Replies"))

o = s:taboption("advanced", Value, "nftset_name", translate("NFT Set"))
o.validate = helpers.validateNftset

o = s:taboption("advanced", Value, "addition_flag", translate("Additional Rule Arguments"))

return m

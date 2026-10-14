-- SPDX-License-Identifier: GPL-3.0-only
local dispatcher = require "luci.dispatcher"
local http = require "luci.http"
local helpers = require "luci.model.smartdns_rs"
local m = Map("smartdns", translate("IP Rules"))
m.template = "smartdns/map"
m.redirect = dispatcher.build_url("admin", "services", "smartdns")

if not arg[1] or m.uci:get("smartdns", arg[1]) ~= "ip-rule-list" then
    http.redirect(m.redirect)
    return
end

local s = m:section(NamedSection, arg[1], "ip-rule-list")
s.anonymous = true
s.addremove = false
s:tab("general", translate("General Settings"))
s:tab("advanced", translate("Advanced Settings"))
local o

o = s:taboption("general", Flag, "enabled", translate("Enable"))
o.default = "1"
o.rmempty = false

o = s:taboption("general", Value, "name", translate("Name"))

o = s:taboption("general", DynamicList, "ip_addr", translate("IP Addresses"))
o.datatype = "ipaddr"

o = s:taboption("general", Value, "ip_set_file", translate("IP Set File"))
o.placeholder = "/etc/smartdns/ip-set/"

o = s:taboption("advanced", Flag, "whitelist_ip", translate("Whitelist IP"))

o = s:taboption("advanced", Flag, "blacklist_ip", translate("Blacklist IP"))

o = s:taboption("advanced", Flag, "ignore_ip", translate("Ignore IP"))

o = s:taboption("advanced", Flag, "bogus_nxdomain", translate("Bogus NXDOMAIN"))

o = s:taboption("advanced", DynamicList, "ip_alias", translate("IP Alias Targets"))
o.datatype = "ipaddr(\"nomask\")"

return m

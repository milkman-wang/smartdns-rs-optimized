-- SPDX-License-Identifier: GPL-3.0-only
local helpers = require "luci.model.smartdns_rs"
local m = Map("smartdns", translate("SmartDNS-rs"))
m.template = "smartdns/map"
helpers.apply_files(m)
local s, o

s = m:section(TypedSection, "client-rule", translate("Client Rules"))
s.template = "cbi/tblsection"
s.anonymous = true
s.addremove = true
s.sortable = true
function s.extedit(self, section)
    return luci.dispatcher.build_url("admin", "services", "smartdns", "client", section)
end
function s.create(self, ...)
    local section = TypedSection.create(self, ...)
    if section then luci.http.redirect(self:extedit(section)) end
    return section
end

o = s:option(Flag, "enabled", translate("Enable"))
o.default = "1"
o.rmempty = false

o = s:option(DummyValue, "client_addr", translate("Client Address"))
function o.cfgvalue(self, section)
    local value = m.uci:get("smartdns", section, "client_addr")
    return type(value) == "table" and table.concat(value, ", ") or value
end

o = s:option(DummyValue, "server_group", translate("Server Group"))

s = m:section(TypedSection, "domain-rule", translate("Domain Rules"))
s.anonymous = true
s.addremove = false
s:tab("forward", translate("DNS Forwarding Setting"))
s:tab("block", translate("DNS Block Setting"))
s:tab("address", translate("Domain Address"))

o = s:taboption("forward", Value, "server_group", translate("Server Group"))
m.uci:foreach("smartdns", "server", function(server)
	if server.server_group then o:value(server.server_group) end
end)

o = s:taboption("forward", Value, "speed_check_mode", translate("Speed Check Mode"))
o:value("", translate("Default"))
o:value("ping,tcp:80,tcp:443")
o:value("ping,tcp:443,tcp:80")
o:value("tcp:80,tcp:443,ping")
o:value("tcp:443,tcp:80,ping")
o:value("http:80,https:443,ping")
o:value("none", translate("None"))
o.validate = helpers.validateSpeedModes

o = s:taboption("forward", ListValue, "dualstack_ip_selection", translate("Dual-stack Selection"))
o:value("", translate("Default"))
o:value("yes", translate("Yes"))
o:value("no", translate("No"))

o = s:taboption("forward", Flag, "force_aaaa_soa", translate("Force AAAA SOA"))

o = s:taboption("forward", Value, "ipset_name", translate("Kernel IP Set"))
o.placeholder = "#4:route4,#6:route6"

o = s:taboption("forward", Flag, "no_serve_expired", translate("Disable Stale Replies"))

o = s:taboption("forward", Value, "nftset_name", translate("NFT Set"))
o.validate = helpers.validateNftset

o = s:taboption("forward", Value, "forwarding_domain_set_file", translate("Forwarding Domain File"))
o.placeholder = "/etc/smartdns/domain-set/"

o = s:taboption("forward", Value, "addition_flag", translate("Additional Rule Arguments"))

o = s:taboption("forward", TextValue, "domain_forwarding_list", translate("Forwarding Domain List"), translate("One domain per line."))
o.rows = 12
helpers.text_file(o, "/etc/smartdns/domain-forwarding.list")

o = s:taboption("block", Value, "block_domain_set_file", translate("Block Domain File"))
o.placeholder = "/etc/smartdns/domain-set/"

o = s:taboption("block", TextValue, "domain_block_list", translate("Blocked Domain List"), translate("One domain per line."))
o.rows = 12
helpers.text_file(o, "/etc/smartdns/domain-block.list")

o = s:taboption("address", TextValue, "address_conf", translate("Address Rules"), translate("Native address directives, for example: address /example.com/192.0.2.1"))
o.rows = 16
helpers.text_file(o, "/etc/smartdns/address.conf")

s = m:section(TypedSection, "domain-rule-list", translate("Domain Rule Lists"))
s.template = "cbi/tblsection"
s.anonymous = true
s.addremove = true
s.sortable = true
function s.extedit(self, section)
    return luci.dispatcher.build_url("admin", "services", "smartdns", "domain", section)
end
function s.create(self, ...)
    local section = TypedSection.create(self, ...)
    if section then luci.http.redirect(self:extedit(section)) end
    return section
end

o = s:option(Flag, "enabled", translate("Enable"))
o.default = "1"
o.rmempty = false

o = s:option(DummyValue, "name", translate("Name"))

o = s:option(DummyValue, "domain_list_file", translate("Domain List File"))

o = s:option(DummyValue, "server_group", translate("Server Group"))

o = s:option(DummyValue, "block_domain_type", translate("Block Type"))
function o.cfgvalue(self, section)
    local value = m.uci:get("smartdns", section, "block_domain_type") or ""
    local labels = {all = translate("IPv4 and IPv6"), ipv4 = translate("IPv4"), ipv6 = translate("IPv6")}
    return labels[value] or translate("None")
end

s = m:section(TypedSection, "ip-rule-list", translate("IP Rules"))
s.template = "cbi/tblsection"
s.anonymous = true
s.addremove = true
s.sortable = true
function s.extedit(self, section)
    return luci.dispatcher.build_url("admin", "services", "smartdns", "ip", section)
end
function s.create(self, ...)
    local section = TypedSection.create(self, ...)
    if section then luci.http.redirect(self:extedit(section)) end
    return section
end

o = s:option(Flag, "enabled", translate("Enable"))
o.default = "1"
o.rmempty = false

o = s:option(DummyValue, "name", translate("Name"))

o = s:option(DummyValue, "ip_addr", translate("IP Addresses"))
function o.cfgvalue(self, section)
    local value = m.uci:get("smartdns", section, "ip_addr")
    return type(value) == "table" and table.concat(value, ", ") or value
end

o = s:option(DummyValue, "ip_set_file", translate("IP Set File"))

s = m:section(TypedSection, "ip-rule", translate("IP Blacklist"))
s.anonymous = true
s.addremove = false

o = s:option(TextValue, "blacklist_conf", translate("Blacklist IP Configuration"), translate("Native blacklist-ip directives."))
o.rows = 14
helpers.text_file(o, "/etc/smartdns/blacklist-ip.conf")

return m

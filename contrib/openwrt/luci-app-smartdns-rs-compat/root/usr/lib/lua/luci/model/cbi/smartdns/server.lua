-- SPDX-License-Identifier: GPL-3.0-only
local dispatcher = require "luci.dispatcher"
local http = require "luci.http"
local helpers = require "luci.model.smartdns_rs"
local m = Map("smartdns", translate("Upstream DNS Servers"))
m.template = "smartdns/map"
m.redirect = dispatcher.build_url("admin", "services", "smartdns", "upstream")

if not arg[1] or m.uci:get("smartdns", arg[1]) ~= "server" then
    http.redirect(m.redirect)
    return
end

local s = m:section(NamedSection, arg[1], "server")
s.anonymous = true
s.addremove = false
s:tab("general", translate("General Settings"))
s:tab("advanced", translate("Advanced Settings"))
local o

o = s:taboption("general", Flag, "enabled", translate("Enable"))
o.default = "1"

o = s:taboption("general", Value, "name", translate("Name"))

o = s:taboption("general", Value, "ip", translate("Address or URL"), translate("Enter an IP address, hostname, or a complete DNS URL. The separate port is ignored for complete URLs."))
o.rmempty = false

o = s:taboption("general", Value, "port", translate("Port"))
o.datatype = "port"

o = s:taboption("general", ListValue, "type", translate("Protocol"))
o.default = "udp"
o.rmempty = false
o:value("udp", translate("UDP"))
o:value("tcp", translate("TCP"))
o:value("tls", translate("DNS over TLS"))
o:value("https", translate("DNS over HTTPS"))
o:value("quic", translate("DNS over QUIC"))
o:value("h3", translate("DNS over HTTP/3"))

o = s:taboption("general", Value, "server_group", translate("Server Group"))
m.uci:foreach("smartdns", "server", function(server)
	if server.server_group then o:value(server.server_group) end
end)

o = s:taboption("advanced", Flag, "exclude_default_group", translate("Exclude Default Group"))

o = s:taboption("advanced", Flag, "blacklist_ip", translate("Blacklist IP Filtering"))

o = s:taboption("advanced", Flag, "check_edns", translate("Require EDNS"))

o = s:taboption("advanced", Value, "spki_pin", translate("TLS SPKI Pin (Base64 SHA-256)"))

o = s:taboption("advanced", Value, "tls_host_verify", translate("TLS Hostname Verify"))

o = s:taboption("advanced", Value, "host_name", translate("TLS SNI Name"))

o = s:taboption("advanced", Flag, "no_check_certificate", translate("Disable Certificate Verification"))

o = s:taboption("advanced", Value, "set_mark", translate("Packet Mark"))
o.validate = helpers.validatePacketMark

o = s:taboption("advanced", Flag, "use_proxy", translate("Use Proxy"))

o = s:taboption("advanced", Value, "addition_arg", translate("Additional Server Arguments"))

return m

-- SPDX-License-Identifier: GPL-3.0-only
local m = Map("smartdns", translate("SmartDNS-rs"))
m.template = "smartdns/map"
local s, o

s = m:section(TypedSection, "server", translate("Upstream DNS Servers"))
s.template = "cbi/tblsection"
s.anonymous = true
s.addremove = true
s.sortable = true
function s.extedit(self, section)
    return luci.dispatcher.build_url("admin", "services", "smartdns", "server", section)
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
o = s:option(DummyValue, "ip", translate("Address or URL"))
o = s:option(DummyValue, "type", translate("Protocol"))
local protocols = {
    udp = translate("UDP"), tcp = translate("TCP"),
    tls = translate("DNS over TLS"), https = translate("DNS over HTTPS"),
    quic = translate("DNS over QUIC"), h3 = translate("DNS over HTTP/3")
}
function o.cfgvalue(self, section)
    local protocol = m.uci:get("smartdns", section, "type") or "udp"
    return protocols[protocol] or protocol
end
o = s:option(DummyValue, "server_group", translate("Server Group"))

return m

-- SPDX-License-Identifier: GPL-3.0-only
local sys = require "luci.sys"
local helpers = require "luci.model.smartdns_rs"
local m = Map("smartdns", translate("SmartDNS-rs"))
m.template = "smartdns/map"
helpers.apply_files(m)
local s, o
s = m:section(SimpleSection)
s.template = "smartdns/status"

s = m:section(TypedSection, "smartdns", translate("Settings"))
s.anonymous = true
s.addremove = false
s:tab("general", translate("General Settings"))
s:tab("advanced", translate("Advanced Settings"))
s:tab("second", translate("Second Server Settings"))
s:tab("dns64", translate("DNS64 Server Settings"))
s:tab("proxy", translate("Proxy Server Settings"))
s:tab("custom", translate("Custom Settings"))

o = s:taboption("general", Flag, "enabled", translate("Enable"))
o.default = "0"
o.rmempty = false

o = s:taboption("general", Value, "server_name", translate("Server Name"))
o.rmempty = true
o.datatype = "hostname"

o = s:taboption("general", Value, "port", translate("Local Port"), translate("Port 6053 is recommended with dnsmasq forwarding. Port 53 makes SmartDNS-rs the main DNS listener."))
o.default = "6053"
o.rmempty = false
o.datatype = "port"

o = s:taboption("general", Flag, "auto_set_dnsmasq", translate("Automatically Set Dnsmasq"), translate("Preserves existing dnsmasq settings and restores them when SmartDNS-rs stops."))
o.default = "1"
o.rmempty = false

o = s:taboption("advanced", Value, "speed_check_mode", translate("Speed Check Mode"), translate("Default: ping,tcp:80,tcp:443. Tests address reachability in this order. Use none to disable response address speed checks."))
o.default = "ping,tcp:80,tcp:443"
o.rmempty = false
o:value("ping,tcp:80,tcp:443")
o:value("ping,tcp:443,tcp:80")
o:value("tcp:80,tcp:443,ping")
o:value("tcp:443,tcp:80,ping")
o:value("http:80,https:443,ping")
o:value("none", translate("None"))
o.validate = helpers.validateSpeedModes

o = s:taboption("advanced", ListValue, "response_mode", translate("Response Mode"), translate("First Ping returns after the first successful speed check. Fastest IP waits to compare addresses. Fastest Response returns the upstream answer without response address speed checks."))
o.default = "first-ping"
o.rmempty = false
o:value("first-ping", translate("First Ping"))
o:value("fastest-ip", translate("Fastest IP"))
o:value("fastest-response", translate("Fastest Response"))

o = s:taboption("advanced", Flag, "tcp_server", translate("TCP Server"))
o.default = "1"
o.rmempty = false

o = s:taboption("advanced", Flag, "tls_server", translate("DNS-over-TLS Server"))
o.default = "0"

o = s:taboption("advanced", Value, "tls_server_port", translate("DNS-over-TLS Port"))
o.default = "853"
o.datatype = "port"
o:depends("tls_server", "1")

o = s:taboption("advanced", Flag, "doh_server", translate("DNS-over-HTTPS Server"))
o.default = "0"

o = s:taboption("advanced", Value, "doh_server_port", translate("DNS-over-HTTPS Port"))
o.default = "8443"
o.datatype = "port"
o:depends("doh_server", "1")

o = s:taboption("advanced", Value, "bind_cert", translate("Server Certificate"))
o.placeholder = "/etc/smartdns/server.pem"
o:depends("tls_server", "1")
o:depends("doh_server", "1")

o = s:taboption("advanced", Value, "bind_cert_key", translate("Server Certificate Key"))
o.placeholder = "/etc/smartdns/server-key.pem"
o:depends("tls_server", "1")
o:depends("doh_server", "1")

o = s:taboption("advanced", Value, "bind_cert_key_pass", translate("Certificate Key Password"))
o.password = true
o:depends("tls_server", "1")
o:depends("doh_server", "1")

o = s:taboption("advanced", Flag, "ddr", translate("Advertise Encrypted Listeners (DDR)"))

o = s:taboption("advanced", ListValue, "bind_cert_generate", translate("Automatic Local Certificate Generation"))
o:value("", translate("Default"))
o:value("auto", translate("Automatic"))
o:value("yes", translate("Yes"))
o:value("no", translate("No"))

o = s:taboption("advanced", DynamicList, "bind_cert_san", translate("Certificate Names and IP Addresses"))
o.placeholder = "resolver.home"

o = s:taboption("advanced", Value, "bind_cert_validity_days", translate("Certificate Validity (Days)"))
o.datatype = "uinteger"
o.placeholder = "390"

o = s:taboption("advanced", Value, "bind_cert_root_key_file", translate("Local CA Key File"))
o.placeholder = "/etc/smartdns/smartdns-root-key.pem"

o = s:taboption("advanced", Flag, "ipv6_server", translate("IPv6 Server"))
o.default = "1"
o.rmempty = false

o = s:taboption("advanced", Flag, "bind_device", translate("Bind Device"), translate("Listen on the selected interface and keep loopback listeners for router-local DNS queries."))
o.default = "0"
o.rmempty = false

o = s:taboption("advanced", Value, "bind_device_name", translate("Bind Device Name"))
o.placeholder = "br-lan"
o:depends("bind_device", "1")

o = s:taboption("advanced", Flag, "dualstack_ip_selection", translate("Dual-stack IP Selection"), translate("Compares IPv4 and IPv6 reachability separately and may add DNS latency. Disable to return both families without this comparison."))
o.default = "1"
o.rmempty = false

o = s:taboption("advanced", Flag, "prefetch_domain", translate("Domain Prefetch"))
o.default = "0"

o = s:taboption("advanced", Flag, "serve_expired", translate("Serve Expired"))
o.default = "1"
o.rmempty = false

o = s:taboption("advanced", Value, "serve_expired_ttl", translate("Maximum Stale Lifetime"))
o.datatype = "uinteger"

o = s:taboption("advanced", Value, "serve_expired_reply_ttl", translate("Stale Reply TTL"))
o.datatype = "uinteger"

o = s:taboption("advanced", Value, "serve_expired_prefetch_time", translate("Expired Cache Refresh Interval"))
o.datatype = "uinteger"
o.placeholder = "300"

o = s:taboption("advanced", Value, "cache_size", translate("Cache Size"))
o.default = "4096"
o.datatype = "integer"
o.validate = helpers.validateCacheSize

o = s:taboption("advanced", Value, "cache_mem_size", translate("Cache Memory Budget"))
o.placeholder = "16MiB"

o = s:taboption("advanced", Flag, "cache_persist", translate("Cache Persist"))
o.default = "1"
o.rmempty = false

o = s:taboption("advanced", Value, "cache_file", translate("Cache File"))
o.placeholder = "/etc/smartdns/smartdns.cache"
o:depends("cache_persist", "1")
o.validate = helpers.validateCacheFile

o = s:taboption("advanced", Value, "num_workers", translate("Worker Threads"))
o.datatype = "range(1,128)"
o.placeholder = "2"

o = s:taboption("advanced", Value, "max_query_limit", translate("Maximum Concurrent Queries"))
o.datatype = "uinteger"
o.placeholder = "0"

o = s:taboption("advanced", Flag, "resolve_local_hostnames", translate("Resolve Local Hostnames"))
o.default = "1"
o.rmempty = false

o = s:taboption("advanced", Value, "domain", translate("Local Domain Suffix"))
o.placeholder = "home"

o = s:taboption("advanced", Value, "odhcpd_lease_file", translate("Odhcpd Lease File"))
o.placeholder = "/tmp/hosts/odhcpd"

o = s:taboption("advanced", Flag, "mdns_lookup", translate("mDNS Lookup"))
o.default = "0"

o = s:taboption("advanced", Flag, "force_aaaa_soa", translate("Force AAAA SOA"))
o.default = "0"

o = s:taboption("advanced", Flag, "force_https_soa", translate("Force HTTPS SOA"))
o.default = "1"
o.rmempty = false

o = s:taboption("advanced", Value, "ipset_name", translate("Kernel IP Set"))
o.placeholder = "#4:route4,#6:route6"

o = s:taboption("advanced", Value, "nftset_name", translate("NFT Set"))
o.placeholder = "#4:inet#fw4#route4"

o = s:taboption("advanced", ListValue, "ipset_timeout", translate("IP Set Entry Timeouts"))
o:value("", translate("Default"))
o:value("yes", translate("Yes"))
o:value("no", translate("No"))

o = s:taboption("advanced", ListValue, "nftset_timeout", translate("NFT Set Entry Timeouts"))
o:value("", translate("Default"))
o:value("yes", translate("Yes"))
o:value("no", translate("No"))

o = s:taboption("advanced", Value, "ipset_no_speed", translate("IP Sets for Failed Probes"))
o.placeholder = "#4:slow4,#6:slow6"

o = s:taboption("advanced", Value, "nftset_no_speed", translate("NFT Sets for Failed Probes"))
o.placeholder = "#4:inet#fw4#slow4"

o = s:taboption("advanced", ListValue, "nftset_debug", translate("NFT Set Debug Logging"))
o:value("", translate("Default"))
o:value("yes", translate("Yes"))
o:value("no", translate("No"))

o = s:taboption("advanced", Value, "rr_ttl", translate("Domain TTL"))
o.datatype = "uinteger"

o = s:taboption("advanced", Value, "rr_ttl_min", translate("Minimum Domain TTL"))
o.datatype = "uinteger"

o = s:taboption("advanced", Value, "rr_ttl_max", translate("Maximum Domain TTL"))
o.datatype = "uinteger"

o = s:taboption("advanced", Value, "rr_ttl_reply_max", translate("Maximum Reply TTL"))
o.datatype = "uinteger"

o = s:taboption("advanced", DynamicList, "conf_files", translate("Include Config Files"))

o = s:taboption("advanced", DynamicList, "hosts_files", translate("Hosts Files"))

o = s:taboption("advanced", Value, "server_flags", translate("Additional Listener Arguments"))
o.rmempty = true

o = s:taboption("advanced", ListValue, "webui_enable", translate("Enable Built-in WebUI (WebUI Package Required)"))
o:value("", translate("Default"))
o:value("yes", translate("Yes"))
o:value("no", translate("No"))

o = s:taboption("advanced", Value, "webui_bind", translate("Built-in WebUI Listen Address"))
o.placeholder = "127.0.0.1:6080"

o = s:taboption("second", Flag, "seconddns_enabled", translate("Enable Second Server"))
o.default = "0"

o = s:taboption("second", Value, "seconddns_port", translate("Second Server Port"))
o.default = "6553"
o.datatype = "port"
o:depends("seconddns_enabled", "1")

o = s:taboption("second", Flag, "seconddns_tcp_server", translate("TCP Server"))
o.default = "1"
o:depends("seconddns_enabled", "1")

o = s:taboption("second", Value, "seconddns_server_group", translate("Upstream Server Group"))
o:depends("seconddns_enabled", "1")
m.uci:foreach("smartdns", "server", function(server)
	if server.server_group then o:value(server.server_group) end
end)

o = s:taboption("second", Flag, "seconddns_no_speed_check", translate("Skip Speed Check"))
o:depends("seconddns_enabled", "1")

o = s:taboption("second", Flag, "seconddns_no_rule_addr", translate("Skip Address Rules"))
o:depends("seconddns_enabled", "1")

o = s:taboption("second", Flag, "seconddns_no_rule_nameserver", translate("Skip Nameserver Rules"))
o:depends("seconddns_enabled", "1")

o = s:taboption("second", Flag, "seconddns_no_rule_ipset", translate("Skip IP Set and NFT Set Rules"))
o:depends("seconddns_enabled", "1")

o = s:taboption("second", Flag, "seconddns_no_rule_soa", translate("Skip SOA Address Rules"))
o:depends("seconddns_enabled", "1")

o = s:taboption("second", Flag, "seconddns_no_dualstack_selection", translate("Skip Dual-stack Selection"))
o:depends("seconddns_enabled", "1")

o = s:taboption("second", Flag, "seconddns_no_cache", translate("Skip Cache"))
o:depends("seconddns_enabled", "1")

o = s:taboption("second", Flag, "seconddns_force_aaaa_soa", translate("Force AAAA SOA"))
o:depends("seconddns_enabled", "1")

o = s:taboption("second", Flag, "seconddns_force_https_soa", translate("Force HTTPS SOA"))
o:depends("seconddns_enabled", "1")

o = s:taboption("second", Value, "seconddns_ipset_name", translate("Kernel IP Set"))
o.placeholder = "#4:route4,#6:route6"
o:depends("seconddns_enabled", "1")

o = s:taboption("second", Value, "seconddns_nftset_name", translate("NFT Set"))
o.placeholder = "#4:inet#fw4#route4"
o:depends("seconddns_enabled", "1")

o = s:taboption("second", Flag, "seconddns_no_serve_expired", translate("Disable Stale Replies"))
o:depends("seconddns_enabled", "1")

o = s:taboption("second", Value, "seconddns_server_flags", translate("Additional Listener Arguments"))
o:depends("seconddns_enabled", "1")

o = s:taboption("dns64", Value, "dns64", translate("DNS64 Prefix"))
o.datatype = "cidr6"
o.placeholder = "64:ff9b::/96"

o = s:taboption("proxy", Value, "proxy_server", translate("Proxy Server URL"))
o.placeholder = "socks5://127.0.0.1:1080"

o = s:taboption("custom", Flag, "coredump", translate("Enable Coredump"), translate("Allow core dumps through procd. The actual file location follows the kernel core pattern."))
o.default = "0"

o = s:taboption("custom", TextValue, "custom_conf", translate("Native Configuration"), translate("SmartDNS-rs directives in this file are included after generated settings."))
o.rows = 18
helpers.text_file(o, "/etc/smartdns/custom.conf")

o = s:taboption("custom", Button, "_check", translate("Validate Configuration"))
o.inputtitle = translate("Validate")
o.inputstyle = "apply"
function o.write()
	-- A successful check is silent. Preserve the exit status separately from
	-- its output so failures cannot be mistaken for success (or an empty alert).
	local result = sys.exec("/etc/init.d/smartdns check 2>&1; printf '\\n%s' \"$?\"")
	local output, code = result:match("^(.*)\n(%d+)$")
	local valid = tonumber(code) == 0
	m.smartdns_check = {
		valid = valid,
		message = translate(valid and "Configuration is valid." or "Configuration validation failed. Please check the system log."),
		output = (output or result):match("^%s*(.-)%s*$")
	}
end

o = s:taboption("custom", ListValue, "log_syslog", translate("Send Logs to Syslog"))
o:value("", translate("Default"))
o:value("yes", translate("Yes"))
o:value("no", translate("No"))

o = s:taboption("custom", ListValue, "audit_soa", translate("Include SOA in Audit Logs"))
o:value("", translate("Default"))
o:value("yes", translate("Yes"))
o:value("no", translate("No"))

o = s:taboption("custom", ListValue, "audit_console", translate("Audit to Console"))
o:value("", translate("Default"))
o:value("yes", translate("Yes"))
o:value("no", translate("No"))

o = s:taboption("custom", ListValue, "audit_syslog", translate("Audit to Syslog"))
o:value("", translate("Default"))
o:value("yes", translate("Yes"))
o:value("no", translate("No"))

o = s:taboption("custom", ListValue, "debug_save_fail_packet", translate("Capture Malformed DNS Packets"))
o:value("", translate("Default"))
o:value("yes", translate("Yes"))
o:value("no", translate("No"))

o = s:taboption("custom", Value, "debug_save_fail_packet_dir", translate("Malformed Packet Directory"))
o.placeholder = "/tmp/smartdns"

o = s:taboption("custom", ListValue, "log_level", translate("Log Level"))
o.default = "warn"
o:value("fatal", translate("Fatal"))
o:value("error", translate("Error"))
o:value("warn", translate("Warning"))
o:value("notice", translate("Notice"))
o:value("info", translate("Information"))
o:value("debug", translate("Debug"))

o = s:taboption("custom", Value, "log_file", translate("Log File"))
o.default = "/var/log/smartdns/smartdns.log"
o.validate = helpers.validateLogFile

o = s:taboption("custom", Value, "log_size", translate("Log Size"))
o.default = "128K"

o = s:taboption("custom", Value, "log_num", translate("Log Number"))
o.default = "2"
o.datatype = "uinteger"

o = s:taboption("custom", Flag, "enable_audit_log", translate("Enable Audit Log"))
o.default = "0"

o = s:taboption("custom", Value, "audit_log_file", translate("Audit Log File"))
o.default = "/var/log/smartdns/smartdns-audit.log"
o:depends("enable_audit_log", "1")
o.validate = helpers.validateLogFile

o = s:taboption("custom", Value, "audit_log_size", translate("Audit Log Size"))
o.default = "128K"
o:depends("enable_audit_log", "1")

o = s:taboption("custom", Value, "audit_log_num", translate("Audit Log Number"))
o.default = "2"
o.datatype = "uinteger"
o:depends("enable_audit_log", "1")

o = s:taboption("custom", Button, "_view_log", translate("View Log"))
o.inputtitle = translate("Open Log Page")
o.inputstyle = "action"
function o.write()
	luci.http.redirect(luci.dispatcher.build_url("admin", "services", "smartdns", "log"))
end

o = s:taboption("custom", Button, "_restart", translate("Restart Service"))
o.inputtitle = translate("Restart")
o.inputstyle = "apply"
function o.write()
	m.message = sys.exec("/etc/init.d/smartdns restart 2>&1")
end

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

s = m:section(TypedSection, "smartdns", translate("Download Files Setting"))
s.anonymous = true
s.addremove = false

o = s:option(Flag, "enable_auto_update", translate("Enable Auto Update"))
o.default = "0"

o = s:option(ListValue, "auto_update_week_time", translate("Update Day"))
o:value("*", translate("Every Day"))
o:value("0", translate("Sunday"))
o:value("1", translate("Monday"))
o:value("2", translate("Tuesday"))
o:value("3", translate("Wednesday"))
o:value("4", translate("Thursday"))
o:value("5", translate("Friday"))
o:value("6", translate("Saturday"))
o:depends("enable_auto_update", "1")

o = s:option(Value, "auto_update_day_time", translate("Update Hour"))
o.default = "5"
o.datatype = "range(0,23)"
o:depends("enable_auto_update", "1")

o = s:option(Value, "_upload_name", translate("File Name"))
o.validate = helpers.validateDownloadName
function o.cfgvalue() return "" end
function o.write() end
function o.remove() end

o = s:option(FileUpload, "upload_conf_file", translate("Upload Config File"))
helpers.upload_file(o, "/etc/smartdns/conf.d")
o = s:option(FileUpload, "upload_list_file", translate("Upload Domain List File"))
helpers.upload_file(o, "/etc/smartdns/domain-set")
o = s:option(FileUpload, "upload_other_file", translate("Upload File"))
helpers.upload_file(o, "/etc/smartdns")

o = s:option(Button, "_update", translate("Update Files Now"))
o.inputtitle = translate("Update")
o.inputstyle = "apply"
function o.write()
	m.message = sys.exec("/etc/init.d/smartdns updatefiles 2>&1")
end

s = m:section(TypedSection, "download-file", translate("Download Files"), translate("Downloaded domain lists take effect only after their /etc/smartdns/domain-set/NAME path is selected in a domain forwarding or blocking rule."))
s.template = "cbi/tblsection"
s.anonymous = true
s.addremove = true
s.sortable = true

o = s:option(Value, "name", translate("File Name"))
o.size = 16
o.rmempty = false
o.validate = helpers.validateDownloadName

o = s:option(Value, "url", translate("URL"))
o.size = 30
o.rmempty = false
o.validate = helpers.validateURL

o = s:option(ListValue, "type", translate("Type"))
o.rmempty = false
o:value("list", translate("Domain List"))
o:value("config", translate("Configuration"))
o:value("ip-set", translate("IP Set"))
o:value("hosts", translate("Hosts"))
o:value("other", translate("Other"))

o = s:option(Value, "desc", translate("Description"))
o.size = 16

o = s:option(Flag, "use_proxy", translate("Use Proxy"))

return m

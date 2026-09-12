-- SPDX-License-Identifier: GPL-3.0-only
local sys = require "luci.sys"
local m = SimpleForm("smartdns_log", translate("SmartDNS-rs Log"))
m.reset = false
m.submit = false
local s = m:section(SimpleSection)
local o = s:option(TextValue, "_log")
o.rows = 30
o.readonly = true
function o.cfgvalue()
	return sys.exec("/usr/libexec/smartdns-rs-call tail 2>&1")
end
o = s:option(Button, "_clear", translate("Clear Logs"))
o.inputstyle = "remove"
function o.write()
	m.message = sys.exec("/usr/libexec/smartdns-rs-call clear_log 2>&1")
end
return m

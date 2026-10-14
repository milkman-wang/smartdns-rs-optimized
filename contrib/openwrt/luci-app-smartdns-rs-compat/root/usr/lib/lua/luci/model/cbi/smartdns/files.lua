-- SPDX-License-Identifier: GPL-3.0-only
local sys = require "luci.sys"
local helpers = require "luci.model.smartdns_rs"
local m = Map("smartdns", translate("SmartDNS-rs"))
m.template = "smartdns/map"
helpers.apply_files(m)
local s, o

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

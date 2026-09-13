-- Run from the repository root with Lua 5.1. No router or running service needed.
local root = "contrib/openwrt/luci-app-smartdns-rs-compat/root/usr/lib/lua/luci/"
package.path = root .. "../?.lua;" .. package.path
local files, commands = {}, {}
package.preload["nixio.fs"] = function()
	return {
		readfile = function(path) return files[path] end,
		writefile = function(path, value) files[path] = value; return #value end,
		mkdirr = function() return true end,
		rename = function(source, target)
			assert(files[source]); files[target] = files[source]; files[source] = nil
			return true
		end
	}
end
translate = function(value) return value end
package.preload["luci.i18n"] = function() return {translate = translate} end
package.preload["luci.sys"] = function()
	return {exec = function(command) commands[#commands + 1] = command; return "command output" end}
end

local helpers = require "luci.model.smartdns_rs"
local cases = {
    {"validateSpeedModes", "ping,tcp-syn:443", true},
    {"validateCacheSize", "-1", true},
    {"validateCacheSize", "0", true},
    {"validateCacheSize", "4096", true},
    {"validateCacheSize", "-2", false},
	{"validateSpeedModes", "ping,tcp:80,tcp:443", true},
	{"validateSpeedModes", "none", true},
	{"validateSpeedModes", "ping,", false},
	{"validateSpeedModes", "tcp:abc", false},
	{"validateNftset", "#4:inet#fw4#dns4,#6:inet#fw4#dns6", true},
	{"validateNftset", "#4:invalid#fw4#dns4", false},
	{"validateCacheFile", "/etc/smartdns/test.cache", true},
	{"validateCacheFile", "/etc/smartdns/test.txt", false},
	{"validateLogFile", "/var/log/smartdns/smartdns.log", true},
	{"validateLogFile", "/etc/config/smartdns", false},
	{"validateDownloadName", "antiad.txt", true},
	{"validateDownloadName", "../antiad.txt", false},
	{"validatePacketMark", "0xffffffff", true},
	{"validatePacketMark", "4294967295", true},
	{"validatePacketMark", "4294967296", false},
	{"validatePacketMark", "-1", false},
	{"validateURL", "https://example.com/rules.txt", true},
	{"validateURL", "not-a-url", false}
}
for _, case in ipairs(cases) do
	local value, err = helpers[case[1]]({}, case[2])
	if case[3] then assert(value == case[2], case[1])
	else assert(value == nil and err, case[1]) end
end

-- Exercise CBI model construction and handlers using its public form API.
local methods = {}
local function object(kind, name, title)
	return setmetatable({kind = kind, name = name, title = title, options = {},
		sections = {}, enabled = "1", disabled = "0"}, {__index = methods})
end
function methods:section(kind, name, title)
	local section = object(kind, name, title)
	section.map = self
	self.sections[#self.sections + 1] = section
	return section
end
function methods:option(kind, name, title)
	local option = object(kind, name, title)
	option.map = self.map
	self.options[name] = option
	return option
end
function methods:taboption(tab, ...) return self:option(...) end
function methods:tab() end
function methods:depends() end
function methods:value() end
function methods:chain() end
function methods:cbid(section) return "cbid.smartdns." .. section .. "." .. self.name end
for _, kind in ipairs({"TypedSection", "NamedSection", "SimpleSection", "Flag", "Value", "DummyValue", "ListValue",
	"DynamicList", "FileUpload", "TextValue", "Button"}) do _G[kind] = kind end
function Map(name, title)
	local map = object("Map", name, title)
	map.uci = {foreach = function() end}
	return map
end
SimpleForm = Map
local model = assert(loadfile(root .. "model/cbi/smartdns/smartdns.lua"))()
local function option(section_name, option_name)
	for _, section in ipairs(model.sections) do
		if section.name == section_name and section.options[option_name] then
			return section.options[option_name]
		end
	end
	error("Missing option: " .. section_name .. "." .. option_name)
end
assert(option("smartdns", "port").default == "6053")
assert(option("smartdns", "enabled").default == "0")
assert(option("smartdns", "speed_check_mode").default == "ping,tcp:80,tcp:443")
assert(option("smartdns", "response_mode").default == "first-ping")
local upstream = option("server", "enabled").map
local server_table
for _, section in ipairs(upstream.sections) do
	if section.name == "server" then server_table = section end
end
assert(server_table.template == "cbi/tblsection")
local columns = 0
for _ in pairs(server_table.options) do columns = columns + 1 end
assert(columns == 5, "Upstream list must contain summary columns only")
assert(option("server", "ip").kind == DummyValue)
assert(server_table.options.set_mark == nil, "Advanced settings belong on the edit page")
assert(option("download-file", "name").validate == helpers.validateDownloadName)
local editor = option("smartdns", "custom_conf")
editor:write("cfg01", "cache-size 8192\r\n")
assert(files["/etc/smartdns/custom.conf"] == "cache-size 8192\n")
assert(editor:cfgvalue() == "cache-size 8192\n")
editor:remove("cfg01")
assert(files["/etc/smartdns/custom.conf"] == "")

local upload = option("smartdns", "upload_list_file")
local source = "/etc/luci-uploads/" .. upload:cbid("cfg01")
model.formvalue = function() return "antiad.txt" end
files[source] = "example.com\n"
assert(upload:validate(source, "cfg01") == source)
upload:write("cfg01", source)
upload:write("cfg01", source) -- CBI reparses after apply.
assert(files["/etc/smartdns/domain-set/antiad.txt"] == "example.com\n")
assert(upload:validate("/etc/config/network", "cfg01") == nil)
model.formvalue = function() return "../escape" end
assert(upload:validate(source, "cfg01") == nil)

option("smartdns", "_check"):write()
assert(commands[#commands] == "/etc/init.d/smartdns check 2>&1")
assert(model.message == "command output")
local log = assert(loadfile(root .. "model/cbi/smartdns/log.lua"))()
assert(log.sections[1].options._log:cfgvalue() == "command output")
assert(commands[#commands] == "/usr/libexec/smartdns-rs-call tail 2>&1")
log.sections[1].options._clear:write()
assert(commands[#commands] == "/usr/libexec/smartdns-rs-call clear_log 2>&1")
assert(loadfile(root .. "controller/smartdns.lua"))

-- The row editor must load exactly one upstream and retain all advanced fields.
local redirected
local dispatcher = {build_url = function(...) return "/" .. table.concat({...}, "/") end}
local http = {redirect = function(url) redirected = url end}
package.preload["luci.dispatcher"] = function() return dispatcher end
package.preload["luci.http"] = function() return http end
luci = {dispatcher = dispatcher, http = http}
assert(server_table:extedit("cfg01") == "/admin/services/smartdns/server/cfg01")
TypedSection = {create = function() return "cfgnew" end}
assert(server_table:create() == "cfgnew")
assert(redirected == "/admin/services/smartdns/server/cfgnew")
local make_map = Map
Map = function(...)
	local map = make_map(...)
	map.uci.get = function(self, config, id, key)
		if id == "cfg01" then return "server" end
		return "smartdns"
	end
	return map
end
arg = {"cfg01"}
local detail = assert(loadfile(root .. "model/cbi/smartdns/server.lua"))()
assert(#detail.sections == 1 and detail.sections[1].name == "cfg01")
assert(detail.sections[1].kind == NamedSection)
assert(detail.sections[1].options.ip.rmempty == false)
assert(detail.sections[1].options.set_mark.validate == helpers.validatePacketMark)
assert(detail.sections[1].options.no_check_certificate.kind == Flag)
assert(detail.sections[1].options.use_proxy.kind == Flag)
assert(detail.sections[1].options.port.datatype == "port")
arg = {"settings"}
assert(assert(loadfile(root .. "model/cbi/smartdns/server.lua"))() == nil)
assert(redirected == "/admin/services/smartdns")
print("Lua 5.1 LuCI models, validation, file handlers and actions: OK")

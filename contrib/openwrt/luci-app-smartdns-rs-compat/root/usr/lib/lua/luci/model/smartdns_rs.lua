-- SPDX-License-Identifier: GPL-3.0-only
local fs = require "nixio.fs"
local i18n = require "luci.i18n"
-- A separate catalog avoids file ownership clashes with older split packages.
-- Ucode-based LuCI loads all language catalogs in its dispatcher instead.
if i18n.loadc then i18n.loadc("smartdns-rs-compat") end
local translate = i18n.translate
local M = {}

function M.text_file(option, path)
	function option.cfgvalue() return fs.readfile(path) or "" end
	-- Browsers submit CRLF line breaks, so even an untouched editor differs
	-- from the file. Only write real changes.
	function option.write(self, section, value)
		value = (value or ""):gsub("\r\n", "\n")
		if value ~= (fs.readfile(path) or "") then
			assert(fs.writefile(path, value))
		end
	end
	function option.remove(self, section) self:write(section, "") end
end

-- File editors save while the form is parsed, which leaves Save & Apply no
-- UCI change to apply: it would report nothing to apply and skip the reload.
-- Reload the service instead, as the JavaScript UI does.
function M.apply_files(map)
	function map.commit_handler(self)
		local http, sys = require "luci.http", require "luci.sys"
		if not http.formvalue("cbi.apply") then return end
		for _, config in ipairs(self.parsechain) do
			-- UCI changes reload the service through procd.
			if next(self.uci:changes(config) or {}) then return end
		end
		self.apply_needed = false
		local ok = sys.call("/etc/init.d/smartdns reload >/dev/null 2>&1") == 0
		self.message = translate(ok and "Configuration changes applied."
			or "Configuration validation failed. Please check the system log.")
	end
end

-- CBI stages multipart uploads here on both Lua LuCI and luci-compat.
-- Keep uploaded files in the same directories used by the JavaScript UI.
function M.upload_file(option, directory)
	-- Modern cbi/upload renders a remote file browser, whose selected paths do
	-- not use CBI's multipart staging. Use a local picker on both LuCI versions.
	option.template = "smartdns/upload"
	function option.cfgvalue() return nil end
	function option.validate(self, value, section)
		if not value or value == "" then return value end
		local name = self.map:formvalue("cbid.smartdns." .. section .. "._upload_name")
		local valid, err = M.validateDownloadName(self, name)
		if not valid then return nil, err end
		if value ~= "/etc/luci-uploads/" .. self:cbid(section) then
			return nil, translate("Upload File")
		end
		return value
	end
	function option.write(self, section, value)
		if not value or value == "" then return end
		-- CBI reparses the form after applying changes; the file has already moved.
		if self.uploaded == value then return end
		local name = self.map:formvalue("cbid.smartdns." .. section .. "._upload_name")
		fs.mkdirr(directory)
		assert(fs.rename(value, directory .. "/" .. name))
		self.uploaded = value
	end
	function option.remove() end
end

function M.validateURL(self, value)
	if value and (value:match("^https?://[^/%s]+") or value:match("^ftp://[^/%s]+")) then
		return value
	end
	return nil, translate("URL")
end

function M.validateSpeedModes(self, value)
	if not value or value == "" or value == "none" then return value end
	for mode in (value .. ","):gmatch("(.-),") do
		local protocol, port = mode:match("^([%a%-]+):(%d+)$")
		if mode ~= "ping" and mode ~= "http" and mode ~= "https" and
			not (port and (protocol == "tcp" or protocol == "tcp-syn" or protocol == "http" or protocol == "https")) then
			return nil, translate("Supported modes are ping, tcp:PORT, tcp-syn:PORT, http[:PORT], https[:PORT], and none.")
		end
	end
	return value
end

function M.validateCacheSize(self, value)
    if value and (value == "-1" or value:match("^%d+$")) then return value end
    return nil, translate("Cache size must be -1, 0, or a positive integer.")
end

function M.validateNftset(self, value)
	if not value or value == "" then return value end
	for set in (value .. ","):gmatch("(.-),") do
		local family = set:match("^#[46]:(%w+)#[%w_%-]+#[%w_%-]+$")
		if family ~= "inet" and family ~= "ip" and family ~= "ip6" then
			return nil, translate("NFT set format: #4:family#table#set,#6:family#table#set")
		end
	end
	return value
end

function M.validateCacheFile(self, value)
	if not value or value == "" or value:match("^/etc/smartdns/[^/]+%.cache$") then return value end
	return nil, translate("Cache file must be /etc/smartdns/NAME.cache.")
end

function M.validateLogFile(self, value)
	if value and value:match("^/var/log/smartdns/[^/]+$") then return value end
	return nil, translate("Log files must be directly under /var/log/smartdns/.")
end

function M.validateDownloadName(self, value)
	if value and value:match("^[A-Za-z0-9][A-Za-z0-9._%-]*$") then return value end
	return nil, translate("File names may contain only letters, numbers, dots, underscores, and hyphens, and must not start with a dot.")
end

function M.validatePacketMark(self, value)
	if not value or value == "" then return value end
	local number
	if value == "0" or value:match("^[1-9][0-9]*$") then number = tonumber(value)
	elseif value:match("^0[xX][0-9a-fA-F]+$") then number = tonumber(value:sub(3), 16) end
	if number and number <= 4294967295 then return value end
	return nil, translate("Packet mark must be a 32-bit decimal or hexadecimal number.")
end

return M

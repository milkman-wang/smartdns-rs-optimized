-- SPDX-License-Identifier: GPL-3.0-only
local fs = require "nixio.fs"
local translate = require("luci.i18n").translate
local M = {}

function M.text_file(option, path)
	function option.cfgvalue() return fs.readfile(path) or "" end
	function option.write(self, section, value)
		assert(fs.writefile(path, (value or ""):gsub("\r\n", "\n")))
	end
	function option.remove(self, section) self:write(section, "") end
end

-- CBI stages multipart uploads here on both Lua LuCI and luci-compat.
-- Keep uploaded files in the same directories used by the JavaScript UI.
function M.upload_file(option, directory)
	function option.cfgvalue() return "" end
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

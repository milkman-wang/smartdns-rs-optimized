// Exercise the LuCI view with its RPC boundaries, without requiring a router.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const source = fs.readFileSync(path.join(__dirname,
    '../luci-app-smartdns-rs/htdocs/luci-static/resources/view/smartdns/smartdns.js'), 'utf8');

function fixture(changes = {}, result = { code: 0 }) {
    const calls = [];
    const api = {
        write: async (file, value) => calls.push(['write', file, value]),
        exec: async (file, args) => { calls.push(['exec', file, args]); return result; }
    };
    const ui = {
        changes: { apply: async checked => calls.push(['apply', checked]) },
        addNotification: (title, node, type) => calls.push(['notice', type])
    };
    const exported = new Function('view', 'rpc', 'fs', 'uci', 'ui', 'E', '_', 'form',
        source.replace('return view.extend({', 'return { textFileOption, page: view.extend({')
            .replace(/\}\);\s*$/, '}) };'))(
        { extend: value => value }, { declare: () => () => {} }, api,
        { changes: async () => changes }, ui, () => ({}), s => s, { TextValue: {} });
    exported.page.handleSave = async () => { calls.push(['save']); };
    return { ...exported, calls };
}

(async () => {
    for (const mode of ['0', '1']) {
        const f = fixture();
        await f.page.handleSaveApply({}, mode);
        assert.deepEqual(f.calls, [
            ['save'], ['exec', '/etc/init.d/smartdns', ['reload']], ['notice', 'info']
        ], 'File-only normal and forced apply must reload even without UCI deltas');
    }
    const mixed = fixture({ smartdns: [['set', 'cfg', 'enabled', '1']] });
    await mixed.page.handleSaveApply({}, '0');
    assert.deepEqual(mixed.calls, [['save'], ['apply', true]],
        'SmartDNS UCI changes must keep the normal checked apply and procd trigger');

    const unrelated = fixture({ network: [['set', 'lan', 'mtu', '1500']] });
    await unrelated.page.handleSaveApply({}, '1');
    assert.deepEqual(unrelated.calls, [
        ['save'], ['exec', '/etc/init.d/smartdns', ['reload']], ['apply', false]
    ], 'Unrelated UCI changes do not replace the file-only reload');

    const invalid = fixture({}, { code: 1, stderr: 'bad configuration' });
    await assert.rejects(invalid.page.handleSaveApply({}, '0'), /bad configuration/);
    assert.equal(invalid.calls.at(-1)[1], 'error');
    assert.ok(!invalid.calls.some(c => c[0] === 'apply'));

    const failedSave = fixture();
    failedSave.page.handleSave = async () => { throw new Error('write failed'); };
    await assert.rejects(failedSave.page.handleSaveApply({}, '0'), /write failed/);
    assert.ok(!failedSave.calls.some(c => c[0] === 'exec'));

    const editor = fixture();
    const option = editor.textFileOption({ option: () => ({}) },
        null, 'custom_conf', 'Custom', '', '/etc/smartdns/custom.conf', 18);
    await option.write('cfg', '#cname /test.invalid/old.invalid\r\naddress /local.invalid/192.0.2.1');
    await option.remove('cfg');
    assert.deepEqual(editor.calls, [
        ['write', '/etc/smartdns/custom.conf', '#cname /test.invalid/old.invalid\naddress /local.invalid/192.0.2.1\n'],
        ['write', '/etc/smartdns/custom.conf', '']
    ], 'Saving preserves comment markers; clearing the editor empties the file');
    console.log('LuCI file-only, mixed, forced, failed and empty-file apply cases: OK');
})().catch(err => { console.error(err); process.exitCode = 1; });

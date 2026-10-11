import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { runInNewContext } from "node:vm";

const html = readFileSync(new URL("../index.html", import.meta.url), "utf8");

function openDemo(protocol) {
    const requests = [];
    const status = { textContent: "", append: (...items) => status.items = items };
    const document = {
        getElementById: () => status,
        createElement: (tag) => ({ tag, addEventListener(type, handler) { this[type] = handler; } }),
        body: { append: (element) => requests.push(element) },
    };
    for (const match of html.matchAll(/<script\b([^>]*)>([\s\S]*?)<\/script>/g)) {
        const src = match[1].match(/\bsrc="([^"]+)"/);
        if (src) requests.push({ src: src[1] });
        else runInNewContext(match[2], { document, location: { protocol } });
    }
    return { requests, status };
}

test("double click explains file restrictions without requesting external modules", () => {
    const { requests, status } = openDemo("file:");
    assert.equal(requests.length, 0);
    assert.match(status.textContent, /file:\/\//);
    assert.equal(status.items[0].href, "../../artifacts/visor-web-local/index.html");
    assert.match(status.items[0].textContent, /visor local/i);
});

for (const protocol of ["http:", "https:"]) {
    test(`${protocol} loads the development module from the same directory`, () => {
        const { requests, status } = openDemo(protocol);
        assert.equal(requests.length, 1);
        assert.equal(requests[0].type, "module");
        assert.equal(requests[0].src, "./main.js");
        requests[0].error();
        assert.match(status.textContent, /README/);
    });
}

/**
 * markdown.js - Safe Markdown rendering via marked.js + highlight.js
 *
 * Expects the host page to load:
 *   <script src="https://cdn.jsdelivr.net/npm/marked/marked.min.js"></script>
 *   <script src="https://cdn.jsdelivr.net/npm/highlight.js/lib/highlight.min.js"></script>
 */

// ------------------------------------------------------------------ XSS guard

/**
 * Escape HTML special characters to prevent XSS.
 * @param {string} text
 * @returns {string}
 */
export function escapeHtml(text) {
    const map = { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#039;' };
    return String(text).replace(/[&<>"']/g, (c) => map[c]);
}

// ------------------------------------------------------------------ highlight

/**
 * Highlight a code block using highlight.js if available.
 * Falls back to plain escaped text.
 * @param {string} code
 * @param {string|null} lang
 * @returns {{ value: string, language: string }}
 */
function highlight(code, lang) {
    if (typeof hljs === 'undefined') {
        return { value: escapeHtml(code), language: lang || 'plaintext' };
    }

    if (lang && hljs.getLanguage(lang)) {
        try {
            return hljs.highlight(code, { language: lang, ignoreIllegals: true });
        } catch {
            // fall through
        }
    }

    return hljs.highlightAuto(code);
}

// ------------------------------------------------------------------ renderer

function buildRenderer() {
    const renderer = new marked.Renderer();

    // Code blocks with syntax highlighting
    renderer.code = (code, lang) => {
        const { value, language } = highlight(code, lang);
        const langClass = language ? ` class="language-${escapeHtml(language)}"` : '';
        return `<pre class="code-block"><code${langClass}>${value}</code></pre>`;
    };

    // Inline code
    renderer.codespan = (code) =>
        `<code class="inline-code">${escapeHtml(code)}</code>`;

    // Links — open external in new tab, sanitize href
    renderer.link = (href, title, text) => {
        const safeHref = sanitizeHref(href);
        const titleAttr = title ? ` title="${escapeHtml(title)}"` : '';
        const external = safeHref && !safeHref.startsWith('#');
        const rel = external ? ' rel="noopener noreferrer"' : '';
        const target = external ? ' target="_blank"' : '';
        return `<a href="${safeHref}"${titleAttr}${target}${rel}>${text}</a>`;
    };

    // Tables
    renderer.table = (header, body) =>
        `<div class="table-wrapper"><table><thead>${header}</thead><tbody>${body}</tbody></table></div>`;

    return renderer;
}

/** Only allow http/https/# hrefs */
function sanitizeHref(href) {
    if (!href) return '#';
    try {
        const url = new URL(href, window.location.href);
        if (url.protocol === 'javascript:') return '#';
        return url.href;
    } catch {
        // relative path or anchor
        return /^[#/]/.test(href) ? href : '#';
    }
}

// ------------------------------------------------------------------ public API

let _configured = false;

function ensureConfigured() {
    if (_configured || typeof marked === 'undefined') return;

    marked.setOptions({
        renderer: buildRenderer(),
        gfm: true,
        breaks: true,
        smartypants: false,
        // Disable built-in highlight in favour of our wrapper
        highlight: null,
    });

    _configured = true;
}

/**
 * Render Markdown to safe HTML.
 * @param {string} text  Raw markdown string
 * @returns {string}     HTML string, safe for innerHTML assignment
 */
export function renderMarkdown(text) {
    if (!text) return '';

    if (typeof marked === 'undefined') {
        // marked.js not loaded — fall back to pre-escaped plain text
        return `<pre>${escapeHtml(text)}</pre>`;
    }

    ensureConfigured();

    // marked.parse returns a string; it uses our custom renderer which
    // escapes user content, so innerHTML assignment is safe.
    return marked.parse(String(text));
}

/**
 * Render inline Markdown (no block-level elements).
 * @param {string} text
 * @returns {string}
 */
export function renderInlineMarkdown(text) {
    if (!text) return '';
    if (typeof marked === 'undefined') return escapeHtml(text);
    ensureConfigured();
    return marked.parseInline(String(text));
}

export default renderMarkdown;

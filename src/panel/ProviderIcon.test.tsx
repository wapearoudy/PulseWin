import { describe, expect, it } from 'vitest'
import { renderToStaticMarkup } from 'react-dom/server'
import { ProviderIcon, rememberExtensionIcons } from './ProviderIcon'

// A 1×1 PNG, the same shape Rust hands over once it has checked the file.
const PNG = 'data:image/png;base64,iVBORw0KGgo='

describe('ProviderIcon', () => {
  it("draws an extension's own PNG in the colours it was drawn in", () => {
    rememberExtensionIcons([{ accountId: 'extension--account-6f776e', icon: PNG }])
    const html = renderToStaticMarkup(<ProviderIcon provider="extension--account-6f776e" />)
    expect(html).toContain('data-original-colour="true"')
    expect(html).toContain(PNG)
    // The artwork is the artwork: no mask, no tint.
    expect(html).not.toContain('mask')
    expect(html).not.toContain('currentColor')
  })

  it('gives an extension with no icon of its own the default glyph', () => {
    rememberExtensionIcons([{ accountId: 'extension--account-6e6f6e65', icon: null }])
    const html = renderToStaticMarkup(<ProviderIcon provider="extension--account-6e6f6e65" />)
    expect(html).toContain('currentColor')
    expect(html).toContain('mask')
    expect(html).not.toContain('data-original-colour')
  })

  it('keeps every built-in provider a monochrome glyph', () => {
    const html = renderToStaticMarkup(<ProviderIcon provider="claude-code--account-31" />)
    expect(html).toContain('currentColor')
    expect(html).toContain('mask')
    expect(html).not.toContain('data-original-colour')
  })
})

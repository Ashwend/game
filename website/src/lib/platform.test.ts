import { describe, expect, it } from 'vitest'
import { mobileFromHint, platformFromHint } from './platform'

const ua = {
  windowsChrome:
    'windows mozilla/5.0 (windows nt 10.0; win64; x64) applewebkit/537.36 (khtml, like gecko) chrome/126.0.0.0 safari/537.36',
  macSafari:
    ' mozilla/5.0 (macintosh; intel mac os x 10_15_7) applewebkit/605.1.15 (khtml, like gecko) version/17.5 safari/605.1.15',
  macChrome:
    'macos mozilla/5.0 (macintosh; intel mac os x 10_15_7) applewebkit/537.36 (khtml, like gecko) chrome/126.0.0.0 safari/537.36',
  linuxFirefox:
    ' mozilla/5.0 (x11; linux x86_64; rv:127.0) gecko/20100101 firefox/127.0',
  iphoneSafari:
    ' mozilla/5.0 (iphone; cpu iphone os 17_5 like mac os x) applewebkit/605.1.15 (khtml, like gecko) version/17.5 mobile/15e148 safari/604.1',
  androidChrome:
    'android mozilla/5.0 (linux; android 14; pixel 8) applewebkit/537.36 (khtml, like gecko) chrome/126.0.0.0 mobile safari/537.36',
  // A hypothetical hint carrying the kernel name; must not read as Windows.
  darwinKernel: ' some-agent/1.0 (darwin 23.4.0; arm64)',
}

describe('platformFromHint', () => {
  it('classifies the mainstream desktop browsers', () => {
    expect(platformFromHint(ua.windowsChrome)).toBe('windows')
    expect(platformFromHint(ua.macSafari)).toBe('macos')
    expect(platformFromHint(ua.macChrome)).toBe('macos')
    expect(platformFromHint(ua.linuxFirefox)).toBe('linux')
  })

  it('maps Apple and Android phones onto the matching desktop OS', () => {
    expect(platformFromHint(ua.iphoneSafari)).toBe('macos')
    expect(platformFromHint(ua.androidChrome)).toBe('linux')
  })

  it("does not mistake 'darwin' for Windows", () => {
    expect(platformFromHint(ua.darwinKernel)).toBe('macos')
  })

  it('falls back to Windows when the hint says nothing useful', () => {
    expect(platformFromHint('')).toBe('windows')
    expect(platformFromHint('mozilla/5.0 (unknown)')).toBe('windows')
  })
})

describe('mobileFromHint', () => {
  it('flags phones and tablets', () => {
    expect(mobileFromHint(ua.iphoneSafari)).toBe(true)
    expect(mobileFromHint(ua.androidChrome)).toBe(true)
  })

  it('leaves desktops alone', () => {
    expect(mobileFromHint(ua.windowsChrome)).toBe(false)
    expect(mobileFromHint(ua.macSafari)).toBe(false)
    expect(mobileFromHint(ua.linuxFirefox)).toBe(false)
  })
})

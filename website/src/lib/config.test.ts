import { describe, expect, it } from 'vitest'
import {
  absoluteUrl,
  latestDownloadUrl,
  releasesUrl,
  siteConfig,
} from './config'
import { DOWNLOADS } from '#/data/content'

describe('absoluteUrl', () => {
  it('joins a root-relative path onto the site origin', () => {
    expect(absoluteUrl('/img/og.jpg')).toBe(`${siteConfig.siteUrl}/img/og.jpg`)
  })
})

describe('releasesUrl', () => {
  it('points at the repo releases listing', () => {
    expect(releasesUrl()).toBe(`${siteConfig.githubRepoUrl}/releases`)
  })
})

describe('latestDownloadUrl', () => {
  it('builds a latest-release asset link from the asset filename', () => {
    // Sample asset name; the source of truth for release asset names is
    // .github/scripts/release_assets.py.
    expect(latestDownloadUrl('ashwend-x86_64-pc-windows-msvc.zip')).toBe(
      `${siteConfig.githubRepoUrl}/releases/latest/download/ashwend-x86_64-pc-windows-msvc.zip`,
    )
  })

  it('produces a resolvable link for every advertised build', () => {
    for (const build of DOWNLOADS) {
      const url = latestDownloadUrl(build.asset)
      expect(url.startsWith('https://')).toBe(true)
      expect(url).toContain('/releases/latest/download/')
      expect(url.endsWith(build.asset)).toBe(true)
    }
  })
})

import { HeadContent, Scripts, createRootRoute } from '@tanstack/react-router'

import type { ReactNode } from 'react'
import { META } from '#/data/content'
import { absoluteUrl, siteConfig } from '#/lib/config'
import appCss from '../styles.css?url'

const ogImage = absoluteUrl(META.ogImage)

const structuredData = {
  '@context': 'https://schema.org',
  '@type': 'VideoGame',
  name: 'Ashwend',
  description: META.description,
  url: siteConfig.siteUrl,
  image: ogImage,
  genre: ['Survival', 'Open World', 'Multiplayer'],
  gamePlatform: ['PC'],
  operatingSystem: 'Windows, macOS, Linux',
  applicationCategory: 'GameApplication',
  sameAs: [siteConfig.discordInviteUrl, siteConfig.githubRepoUrl],
  author: {
    '@type': 'Organization',
    name: 'Ashwend',
    url: siteConfig.siteUrl,
  },
  offers: {
    '@type': 'Offer',
    price: '0',
    priceCurrency: 'USD',
    availability: 'https://schema.org/LimitedAvailability',
  },
}

export const Route = createRootRoute({
  head: () => ({
    meta: [
      { charSet: 'utf-8' },
      { name: 'viewport', content: 'width=device-width, initial-scale=1' },
      { title: META.title },
      { name: 'description', content: META.description },
      { name: 'theme-color', content: '#0a0e13' },
      { name: 'robots', content: 'index, follow' },
      // Open Graph
      { property: 'og:type', content: 'website' },
      { property: 'og:site_name', content: 'Ashwend' },
      { property: 'og:title', content: META.title },
      { property: 'og:description', content: META.description },
      { property: 'og:url', content: siteConfig.siteUrl },
      { property: 'og:image', content: ogImage },
      { property: 'og:image:width', content: '1200' },
      { property: 'og:image:height', content: '630' },
      {
        property: 'og:image:alt',
        content: 'A low sun behind misty pines on the plains of Ashwend',
      },
      // Twitter
      { name: 'twitter:card', content: 'summary_large_image' },
      { name: 'twitter:title', content: META.title },
      { name: 'twitter:description', content: META.description },
      { name: 'twitter:image', content: ogImage },
    ],
    links: [
      { rel: 'icon', href: '/favicon.svg', type: 'image/svg+xml' },
      {
        rel: 'icon',
        href: '/favicon.ico',
        sizes: '32x32',
        type: 'image/x-icon',
      },
      { rel: 'apple-touch-icon', href: '/apple-touch-icon.png' },
      { rel: 'canonical', href: siteConfig.siteUrl },
      {
        rel: 'preload',
        href: '/fonts/Cinzel-Bold.woff2',
        as: 'font',
        type: 'font/woff2',
        crossOrigin: 'anonymous',
      },
      { rel: 'stylesheet', href: appCss },
    ],
  }),
  shellComponent: RootDocument,
})

function RootDocument({ children }: { children: ReactNode }) {
  return (
    <html lang="en">
      <head>
        <HeadContent />
        <script
          type="application/ld+json"
          // Static structured data for search engines; safe, no user input.
          dangerouslySetInnerHTML={{ __html: JSON.stringify(structuredData) }}
        />
      </head>
      <body>
        {children}
        <Scripts />
      </body>
    </html>
  )
}

import { createFileRoute } from '@tanstack/react-router'
import { Header } from '#/components/Header'
import { Hero } from '#/components/Hero'
import { Playtest } from '#/components/Playtest'
import { DiscordCta } from '#/components/DiscordCta'
import { Footer } from '#/components/Footer'

export const Route = createFileRoute('/')({ component: Home })

function Home() {
  return (
    <div id="top">
      {/* Keyboard bypass (WCAG 2.4.1): the first focusable element, hidden until
          focused, jumps past the fixed header straight to the content. */}
      <a
        href="#main"
        className="sr-only rounded-full bg-ember-500 px-4 py-2 font-semibold text-ink-950 focus:not-sr-only focus:absolute focus:left-4 focus:top-4 focus:z-[200]"
      >
        Skip to content
      </a>
      <Header />
      <main id="main">
        <Hero />
        <Playtest />
        <DiscordCta />
      </main>
      <Footer />
    </div>
  )
}

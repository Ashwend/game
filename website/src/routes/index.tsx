import { createFileRoute } from '@tanstack/react-router'
import { Header } from '#/components/Header'
import { Hero } from '#/components/Hero'
import { Playtest } from '#/components/Playtest'
import { Gallery } from '#/components/Gallery'
import { Status } from '#/components/Status'
import { DiscordCta } from '#/components/DiscordCta'
import { Footer } from '#/components/Footer'

export const Route = createFileRoute('/')({ component: Home })

function Home() {
  return (
    <div id="top">
      <Header />
      <main>
        <Hero />
        <Playtest />
        <Gallery />
        <Status />
        <DiscordCta />
      </main>
      <Footer />
    </div>
  )
}

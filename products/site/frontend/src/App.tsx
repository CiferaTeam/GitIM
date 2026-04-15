import { Header } from "@/components/layout/Header"
import { Hero } from "@/components/sections/Hero"
import { ValueProps } from "@/components/sections/ValueProps"
import { ProductDemo } from "@/components/sections/ProductDemo"
import { Credits } from "@/components/sections/Credits"
import { Footer } from "@/components/layout/Footer"

function App() {
  return (
    <>
      <Header />
      <main>
        <Hero />
        <ValueProps />
        <ProductDemo />
        <Credits />
      </main>
      <Footer />
    </>
  )
}

export default App

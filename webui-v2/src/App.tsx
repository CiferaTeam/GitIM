import './App.css'
import { Button } from '@/components/ui/button'

function App() {
  return (
    <div className="flex min-h-svh items-center justify-center">
      <div className="text-center space-y-4">
        <h1 className="text-2xl font-semibold">GitIM webui-v2</h1>
        <p className="text-muted-foreground">Scaffold ready. Build starts here.</p>
        <Button>Get Started</Button>
      </div>
    </div>
  )
}

export default App

/*
 * Copyright © 2025-2026 rustmailer.com
 * Licensed under RustMailer License Agreement v1.0
 * Unauthorized use or distribution is prohibited.
 */

import { StrictMode } from 'react'
import ReactDOM from 'react-dom/client'
import { AxiosError } from 'axios'
import {
  QueryCache,
  QueryClient,
  QueryClientProvider,
} from '@tanstack/react-query'
import { RouterProvider, createRouter } from '@tanstack/react-router'
import { resetAccessToken } from '@/stores/authStore'
import { toast } from '@/hooks/use-toast'
import { ThemeProvider } from './context/theme-context'
import './index.css'
// Generated Routes
import { routeTree } from './routeTree.gen'
import { ToastAction } from './components/ui/toast'


const handleAxiosError = (error: any) => {
  if (!(error instanceof AxiosError)) return;

  switch (error.response?.status) {
    case 401:
      toast({
        variant: 'destructive',
        title: 'Session expired!',
        description: 'Your session has ended due to inactivity. Please log in again to continue.',
        action: <ToastAction altText="Close">Close</ToastAction>,
      });
      resetAccessToken();
      const currentPath = router.history.location.pathname;
      if (currentPath !== '/sign-in') {
        const redirect = `${router.history.location.href}`;
        router.navigate({ to: '/sign-in', search: { redirect } });
      }
      break;
    case 403:
      toast({
        variant: 'destructive',
        title: 'Access Denied',
        description: 'You do not have permission to perform this action.',
      });
      break;
    case 500:
      toast({
        variant: 'destructive',
        title: 'Internal Server Error!',
      });
      router.navigate({ to: '/500' });
      break;
    case 304:
      toast({
        variant: 'destructive',
        title: 'Content not modified!',
      });
      break;
    default:
      if (error.code === "ERR_NETWORK") {
        toast({
          variant: "destructive",
          title: "Network Error",
          description: "Unable to connect to the server. Please check your internet connection and try again.",
          action: <ToastAction altText="Try again">Try again</ToastAction>,
        });
      }
  }
};



const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      retry: (failureCount, error) => {
        // eslint-disable-next-line no-console
        if (import.meta.env.DEV) console.log({ failureCount, error })

        if (failureCount >= 0 && import.meta.env.DEV) return false
        if (failureCount > 3 && import.meta.env.PROD) return false

        return !(
          error instanceof AxiosError &&
          [401, 403].includes(error.response?.status ?? 0)
        )
      },
      refetchOnWindowFocus: import.meta.env.PROD,
      staleTime: 10 * 1000, // 10s
    },
    mutations: {
      onError: (error) => {
        handleAxiosError(error)
      },
    },
  },
  queryCache: new QueryCache({
    onError: (error) => {
      handleAxiosError(error)
    },
  }),
})

// Create a new router instance
const router = createRouter({
  routeTree,
  context: { queryClient },
  defaultPreload: 'intent',
  defaultPreloadStaleTime: 0,
})

// Register the router instance for type safety
declare module '@tanstack/react-router' {
  interface Register {
    router: typeof router
  }
}

// Render the app
const rootElement = document.getElementById('root')!
if (!rootElement.innerHTML) {
  const root = ReactDOM.createRoot(rootElement)
  root.render(
    <StrictMode>
      <QueryClientProvider client={queryClient}>
        <ThemeProvider defaultTheme='light' storageKey='vite-ui-theme'>
          <RouterProvider router={router} />
        </ThemeProvider>
      </QueryClientProvider>
    </StrictMode>
  )
}

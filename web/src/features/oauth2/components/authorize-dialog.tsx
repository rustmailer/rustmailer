/*
 * Copyright © 2025-2026 rustmailer.com
 * Licensed under RustMailer License Agreement v1.0
 * Unauthorized use or distribution is prohibited.
 */

import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { OAuth2Entity } from '../data/schema'
import { useNavigate } from '@tanstack/react-router'
import { useState } from 'react'
import { VirtualizedSelect } from '@/components/virtualized-select'
import useMinimalAccountList from '@/hooks/use-minimal-account-list'
import { useMutation } from '@tanstack/react-query'
import { exchange_client_credentials, get_authorize_url } from '@/api/oauth2/api'
import { toast } from '@/hooks/use-toast'
import { ToastAction } from '@/components/ui/toast'
import { AxiosError } from 'axios'
import { Loader2 } from 'lucide-react'

interface Props {
  currentRow: OAuth2Entity
  open: boolean
  onOpenChange: (open: boolean) => void
}


export function AuthorizeDialog({ currentRow, open, onOpenChange }: Props) {
  const navigate = useNavigate()
  const [accountId, setAccountId] = useState<number | null>(null)
  const { accountsOptions, minimalList, isLoading } = useMinimalAccountList();

  const isClientCredentials = currentRow.grant_type === 'ClientCredentials'

  const authorizeMutation = useMutation({
    mutationFn: () =>
      isClientCredentials
        ? exchange_client_credentials(accountId!, currentRow.id)
        : get_authorize_url({ account_id: accountId, oauth2_id: currentRow.id }),
    onSuccess: (url: any) => {
      if (isClientCredentials) {
        toast({
          title: 'Access Token Acquired',
          description: 'Client credentials exchanged successfully. The access token has been stored.',
          action: <ToastAction altText="Close">Close</ToastAction>,
        });
      } else if (typeof url === 'string' && url) {
        window.open(url, '_blank');
      }
      onOpenChange(false);
    },
    onError: handleError
  });

  function handleError(error: AxiosError) {
    const errorMessage = (error.response?.data as { message?: string })?.message ||
      error.message ||
      `get authorize url failed, please try again later`;

    toast({
      variant: "destructive",
      title: isClientCredentials ? 'Client Credentials Exchange Failed' : 'Get Authorize Url Failed',
      description: errorMessage as string,
      action: <ToastAction altText="Try again">Try again</ToastAction>,
    });
    console.error(error);
  }


  function doAuthorize() {
    authorizeMutation.mutate();
  }

  return (
    <Dialog
      open={open}
      onOpenChange={(state) => {
        onOpenChange(state);
      }}
    >
      <DialogContent className='sm:max-w-lg' autoFocus>
        <DialogHeader className='text-left'>
          <DialogTitle>{isClientCredentials ? 'Exchange Client Credentials' : 'Authorize Email Account'}</DialogTitle>
          <DialogDescription>
            {isClientCredentials
              ? 'Exchange client credentials to obtain an access token for the selected account. No user interaction is required.'
              : 'Authorize an email account to start the OAuth2 authorization process.'}
          </DialogDescription>
        </DialogHeader>
        <div className='flex flex-col space-y-4 h-24'>
          {isLoading && <div className="flex justify-center items-center h-full">
            <div>Loading Accounts...</div>
          </div>}
          {!isLoading && minimalList && minimalList.length > 0 && (
            <div className="flex justify-start items-start h-full ml-4">
              <VirtualizedSelect
                isLoading={isLoading}
                className='w-full'
                options={accountsOptions}
                onSelectOption={(values) => setAccountId(parseInt(values[0], 10))}
                placeholder="Select an account"
              />
            </div>
          )}
          {!isLoading && minimalList?.length === 0 && (
            <div className="flex flex-col items-center justify-center h-full">
              <p className="mb-4">No email accounts registered. Please create one.</p>
              <Button onClick={() => {
                navigate({ to: '/accounts' })
              }}>
                Create Account
              </Button>
            </div>
          )}
        </div>
        <DialogFooter>
          <DialogClose asChild>
            <Button variant='outline' className="px-2 py-1 text-sm h-auto">Close</Button>
          </DialogClose>
          {!isLoading && minimalList && minimalList.length > 0 && (
            <Button disabled={!accountId || authorizeMutation.isPending} onClick={doAuthorize}>
              {authorizeMutation.isPending && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
              {isClientCredentials ? 'Exchange' : 'Authorize'}
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

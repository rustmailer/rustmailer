/*
 * Copyright © 2025-2026 rustmailer.com
 * Licensed under RustMailer License Agreement v1.0
 * Unauthorized use or distribution is prohibited.
 */

import { z } from 'zod'
import { useFieldArray, useForm } from 'react-hook-form'
import { zodResolver } from '@hookform/resolvers/zod'
import { toast } from '@/hooks/use-toast'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import {
  Form,
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from '@/components/ui/form'
import { Input } from '@/components/ui/input'
import { ScrollArea } from '@/components/ui/scroll-area'
import { Textarea } from '@/components/ui/textarea'
import { OAuth2Entity } from '../data/schema'
import { Checkbox } from '@/components/ui/checkbox'
import { cn } from '@/lib/utils'
import { Loader2, MinusCircle, Plus } from 'lucide-react'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import { create_oauth2, update_oauth2 } from '@/api/oauth2/api'
import { ToastAction } from '@/components/ui/toast'
import { AxiosError } from 'axios'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import useProxyList from '@/hooks/use-proxy'

const paramSchema = z.object({
  key: z.string({ required_error: 'Key is required' }).min(1, "Key cannot be empty"),
  value: z.string({ required_error: 'Value is required' }).min(1, "Value cannot be empty"),
});

const scopeSchema = z.object({
  value: z.string({ required_error: 'Value is required' }).min(1, "Value cannot be empty"),
});

const extraparamSchema = z.record(z.string()).optional();
const authorizescopeSchema = z.array(z.string()).optional();


function convertToExtraParamsSchema(
  record: z.infer<typeof extraparamSchema>
): z.infer<typeof paramSchema>[] {
  if (!record) {
    return [];
  }
  return Object.entries(record).map(([key, value]) => ({
    key,
    value,
  }));
}


function convertToScopeSchema(authorizeScopes: z.infer<typeof authorizescopeSchema>): z.infer<typeof scopeSchema>[] {
  if (!authorizeScopes || authorizeScopes.length === 0) {
    return [];
  }

  return authorizeScopes.map((scope) => ({
    value: scope,
  }));
}

const grantTypeSchema = z.enum(['AuthorizationCode', 'ClientCredentials'], {
  required_error: 'Grant type is required',
});

const oauth2Schema = z.object({
  description: z.string().max(255, { message: "Description must not exceed 255 characters." }).optional(),
  client_id: z.string({
    required_error: "Client ID is required",
  }).min(1, { message: "Client ID cannot be empty" }),
  client_secret: z.string().optional(),
  grant_type: grantTypeSchema,
  auth_url: z.string({
    required_error: "Authorization URL is required",
  })
    .min(1, { message: "Authorization URL cannot be empty" })
    .url({ message: "Invalid Authorization URL format" }).optional().or(z.literal('')),
  token_url: z.string({
    required_error: "Token URL is required",
  })
    .min(1, { message: "Token URL cannot be empty" })
    .url({ message: "Invalid Token URL format" }),
  redirect_uri: z.string().url({ message: "Invalid Redirect URI format" }).optional().or(z.literal('')),
  scopes: z.array(scopeSchema).optional(),
  extra_params: z.array(paramSchema).optional(),
  enabled: z.boolean(),
  use_proxy: z.number().optional(),
});

export type OAuth2Form = z.infer<typeof oauth2Schema>;


interface Props {
  currentRow?: OAuth2Entity
  open: boolean
  onOpenChange: (open: boolean) => void
}

const defaultValues: OAuth2Form = {
  description: undefined,
  client_id: '',
  client_secret: '',
  grant_type: 'AuthorizationCode',
  auth_url: '',
  token_url: '',
  redirect_uri: '',
  extra_params: [],
  scopes: [],
  enabled: true,
  use_proxy: undefined
};


export function ActionDialog({ currentRow, open, onOpenChange }: Props) {
  const isEdit = !!currentRow
  const form = useForm<OAuth2Form>({
    resolver: zodResolver(oauth2Schema),
    defaultValues: isEdit
      ? {
        description: currentRow.description ?? undefined,
        client_id: currentRow.client_id,
        client_secret: undefined,
        grant_type: currentRow.grant_type,
        auth_url: currentRow.auth_url,
        token_url: currentRow.token_url,
        redirect_uri: currentRow.redirect_uri,
        extra_params: currentRow.extra_params ? convertToExtraParamsSchema(currentRow.extra_params) : undefined,
        scopes: currentRow.scopes ? convertToScopeSchema(currentRow.scopes) : undefined,
        enabled: currentRow.enabled,
        use_proxy: currentRow.use_proxy === null ? undefined : currentRow.use_proxy,
      }
      : defaultValues,
  });

  const { proxyOptions } = useProxyList();

  const { fields: params, append: params_append, remove: params_remove } = useFieldArray({
    name: 'extra_params',
    control: form.control,
  })

  const { fields: scopes, append: scopes_append, remove: scopes_remove } = useFieldArray({
    name: 'scopes',
    control: form.control,
  })


  const queryClient = useQueryClient();

  const createMutation = useMutation({
    mutationFn: create_oauth2,
    onSuccess: handleSuccess,
    onError: handleError
  });

  const updateMutation = useMutation({
    mutationFn: (data: Record<string, any>) => update_oauth2(currentRow?.id!, data),
    onSuccess: handleSuccess,
    onError: handleError
  })



  function handleSuccess() {
    toast({
      title: `OAuth2 ${isEdit ? 'Updated' : 'Created'}`,
      description: `Your OAuth2 application has been successfully ${isEdit ? 'updated' : 'created'}.`,
      action: <ToastAction altText="Close">Close</ToastAction>,
    });

    queryClient.invalidateQueries({ queryKey: ['oauth2-list'] });
    form.reset();
    onOpenChange(false);
  }
  function handleError(error: AxiosError) {
    const errorMessage = (error.response?.data as { message?: string })?.message ||
      error.message ||
      `${isEdit ? 'Update' : 'Creation'} failed, please try again later`;

    toast({
      variant: "destructive",
      title: `OAuth2 ${isEdit ? 'Update' : 'Creation'} Failed`,
      description: errorMessage as string,
      action: <ToastAction altText="Try again">Try again</ToastAction>,
    });
    console.error(error);
  }

  const onSubmit = (values: OAuth2Form) => {
    if (!isEdit) {
      if (!values.client_secret) {
        form.setError('client_secret', {
          type: 'manual',
          message: 'Client Secret is required'
        });
        return;
      }
      if (values.client_secret.length < 1) {
        form.setError('client_secret', {
          type: 'manual',
          message: 'Client Secret cannot be empty'
        });
        return;
      }
    }

    const prepareClientSecret = (secret: string | undefined) => {
      return secret && secret.trim() !== '' ? secret : undefined;
    };

    if (isEdit) {
      updateMutation.mutate({
        description: values.description,
        client_id: values.client_id,
        client_secret: prepareClientSecret(values.client_secret),
        grant_type: values.grant_type,
        auth_url: values.auth_url,
        token_url: values.token_url,
        redirect_uri: values.redirect_uri,
        extra_params: values.extra_params?.reduce((acc, item) => ({ ...acc, [item.key]: item.value }), {}),
        scopes: values.scopes?.map(scope => scope.value),
        enabled: values.enabled,
        use_proxy: values.use_proxy
      });
    } else {
      createMutation.mutate({
        description: values.description,
        client_id: values.client_id,
        client_secret: values.client_secret!,
        grant_type: values.grant_type,
        auth_url: values.auth_url,
        token_url: values.token_url,
        redirect_uri: values.redirect_uri,
        extra_params: values.extra_params?.reduce((acc, item) => ({ ...acc, [item.key]: item.value }), {}),
        scopes: values.scopes?.map(scope => scope.value),
        enabled: values.enabled,
        use_proxy: values.use_proxy
      });
    }
  }

  const isClientCredentials = form.watch('grant_type') === 'ClientCredentials'

  return (
    <Dialog
      open={open}
      onOpenChange={(state) => {
        form.reset()
        onOpenChange(state)
      }}
    >
      <DialogContent className='w-full md:max-w-4xl'>
        <DialogHeader className='text-left mb-4'>
          <DialogTitle>{isEdit ? 'Edit' : 'Add New'}</DialogTitle>
          <DialogDescription>
            {isEdit ? 'Update here. ' : 'Create new here. '}
            Click save when you&apos;re done.
          </DialogDescription>
        </DialogHeader>
        <div className="flex items-center justify-start gap-2 mb-4">
          <span className="text-sm text-muted-foreground mr-2">
            Quick presets:
          </span>
          <Button
            variant="secondary"
            size="sm"
            onClick={() => {
              form.setValue("auth_url", "https://accounts.google.com/o/oauth2/v2/auth");
              form.setValue("token_url", "https://oauth2.googleapis.com/token");
              form.setValue("enabled", true);
              form.setValue("grant_type", "AuthorizationCode");
              form.setValue("redirect_uri", "http://localhost");
              form.setValue("scopes", [{ value: "https://mail.google.com/" }]);
              form.setValue("extra_params", [{ key: "access_type", value: "offline" }, { key: "prompt", value: "consent" }])
            }}
          >
            Gmail
          </Button>

          <Button
            variant="secondary"
            size="sm"
            onClick={() => {
              form.setValue("auth_url", "https://login.microsoftonline.com/consumers/oauth2/v2.0/authorize");
              form.setValue("token_url", "https://login.microsoftonline.com/consumers/oauth2/v2.0/token");
              form.setValue("enabled", true);
              form.setValue("grant_type", "AuthorizationCode");
              form.setValue("redirect_uri", "http://localhost");
              form.setValue("scopes", [{ value: "https://graph.microsoft.com/Mail.ReadWrite" }, { value: "https://graph.microsoft.com/Mail.Send" }, { value: "offline_access" }]);
              form.setValue("extra_params", [{ key: "prompt", value: "consent" }])
            }}
          >
            Outlook (Consumer)
          </Button>

          <Button
            variant="secondary"
            size="sm"
            onClick={() => {
              form.setValue("auth_url", "https://login.microsoftonline.com/common/oauth2/v2.0/authorize");
              form.setValue("token_url", "https://login.microsoftonline.com/common/oauth2/v2.0/token");
              form.setValue("enabled", true);
              form.setValue("grant_type", "ClientCredentials");
              form.setValue("redirect_uri", "http://localhost");
              form.setValue("scopes", [{ value: "https://graph.microsoft.com/.default" }]);
              form.setValue("extra_params", []);
            }}
          >
            Microsoft 365 (Service)
          </Button>
        </div>
        <ScrollArea className='h-[40rem] w-full pr-4 -mr-4 py-1'>
          <Form {...form}>
            <form
              id='oauth2-form'
              onSubmit={form.handleSubmit(onSubmit)}
              className='space-y-4 p-0.5'
            >
              <FormField
                control={form.control}
                name='grant_type'
                render={({ field }) => (
                  <FormItem>
                    <FormLabel>Grant Type:</FormLabel>
                    <Select onValueChange={field.onChange} defaultValue={field.value}>
                      <FormControl>
                        <SelectTrigger>
                          <SelectValue placeholder="Select grant type" />
                        </SelectTrigger>
                      </FormControl>
                      <SelectContent>
                        <SelectItem value="AuthorizationCode">Authorization Code + PKCE (requires user login)</SelectItem>
                        <SelectItem value="ClientCredentials">Client Credentials (no user interaction, service-to-service)</SelectItem>
                      </SelectContent>
                    </Select>
                    <FormDescription>
                      {field.value === 'ClientCredentials'
                        ? 'Uses client_id + client_secret to obtain tokens automatically. Suitable for organizational accounts with Microsoft Graph.'
                        : 'Uses PKCE flow with user interaction. Suitable for personal accounts (Outlook.com, Hotmail).'}
                    </FormDescription>
                    <FormMessage />
                  </FormItem>
                )}
              />
              <FormField
                control={form.control}
                name='enabled'
                render={({ field }) => (
                  <FormItem className='flex flex-row items-center gap-x-2'>
                    <FormControl>
                      <Checkbox
                        className='mt-2'
                        checked={field.value}
                        onCheckedChange={field.onChange}
                      />
                    </FormControl>
                    <FormLabel>Enabled</FormLabel>
                    <FormDescription>
                      When disabled:
                      - New authorization flows will be rejected immediately
                      - Existing access tokens and refresh tokens will be revoked within 1 minute
                      - Users must re-authorize when re-enabled
                    </FormDescription>
                    <FormMessage />
                  </FormItem>
                )}
              />
              <FormField
                control={form.control}
                name='client_id'
                render={({ field }) => (
                  <FormItem className='flex flex-col gap-y-1 space-y-0'>
                    <FormLabel className='mb-1'>Client Id:</FormLabel>
                    <FormControl>
                      <Input
                        placeholder='Enter your client ID'
                        {...field}
                      />
                    </FormControl>
                    <FormDescription>
                      The unique identifier for your application, provided by the OAuth provider.
                    </FormDescription>
                    <FormMessage />
                  </FormItem>
                )}
              />
              <FormField
                control={form.control}
                name='client_secret'
                render={({ field }) => (
                  <FormItem className='flex flex-col gap-y-1 space-y-0'>
                    <FormLabel className='mb-1'>Client Secret:</FormLabel>
                    <FormControl>
                      <Input
                        placeholder={isEdit ? 'Leave empty to keep existing secret' : 'Enter your client secret'}
                        {...field}
                      />
                    </FormControl>
                    <FormDescription>
                      {isEdit
                        ? 'Leave empty to keep the existing secret. Only enter a new value if you want to change it.'
                        : 'A secret key provided by the OAuth provider to authenticate your application.'}
                    </FormDescription>
                    <FormMessage />
                  </FormItem>
                )}
              />
              {!isClientCredentials && (
                <FormField
                  control={form.control}
                  name='auth_url'
                  render={({ field }) => (
                    <FormItem className='flex flex-col gap-y-1 space-y-0'>
                      <FormLabel className='mb-1'>Auth Url:</FormLabel>
                      <FormControl>
                        <Input
                          placeholder='Enter the authorization URL'
                          {...field}
                        />
                      </FormControl>
                      <FormDescription>
                        The URL where users will be redirected to authorize your application.
                      </FormDescription>
                      <FormMessage />
                    </FormItem>
                  )}
                />
              )}
              <FormField
                control={form.control}
                name='token_url'
                render={({ field }) => (
                  <FormItem className='flex flex-col gap-y-1 space-y-0'>
                    <FormLabel className='mb-1'>Token Url:</FormLabel>
                    <FormControl>
                      <Input
                        placeholder='Enter the token URL'
                        {...field}
                      />
                    </FormControl>
                    <FormDescription>
                      The URL used to exchange credentials for an access token.
                      For Microsoft Graph client credentials, replace <code>YOUR_TENANT_ID</code> with your tenant ID or domain (e.g. <code>yourtenant.onmicrosoft.com</code>):{' '}
                      <code>https://login.microsoftonline.com/YOUR_TENANT_ID/oauth2/v2.0/token</code>
                      {' '}Multi-tenant apps may use <code>common</code> or <code>organizations</code> instead.
                    </FormDescription>
                    <FormMessage />
                  </FormItem>
                )}
              />
              {!isClientCredentials && (
                <FormField
                  control={form.control}
                  name='redirect_uri'
                  render={({ field }) => (
                    <FormItem className='flex flex-col gap-y-1 space-y-0'>
                      <FormLabel className='mb-1'>Redirect Url:</FormLabel>
                      <FormControl>
                        <Input
                          placeholder='Enter your redirect URL'
                          {...field}
                        />
                      </FormControl>
                      <FormDescription>
                        The redirect URL after authorization. It must match the one registered with the OAuth provider.
                        Use the format <code>http://[host]:[port]/oauth2/callback</code> (or <code>https://</code>),
                        where <code>[host]</code> and <code>[port]</code> match your RustMailer deployment.
                        The path <code>/oauth2/callback</code> is fixed.
                      </FormDescription>
                      <FormMessage />
                    </FormItem>
                  )}
                />
              )}
              <div>
                {scopes.map((field, index) => (
                  <div className="flex flex-col gap-4 sm:flex-row sm:items-center" key={field.id + index}>
                    <FormField
                      control={form.control}
                      name={`scopes.${index}.value`}
                      render={({ field }) => (
                        <FormItem className="flex-1">
                          <FormLabel className={cn(index !== 0 && "sr-only")}>Scope:</FormLabel>
                          <FormDescription className={cn(index !== 0 && "sr-only")}>
                            Enter the scope here.
                          </FormDescription>
                          <FormControl>
                            <Input {...field} />
                          </FormControl>
                          <FormMessage />
                        </FormItem>
                      )}
                    />
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon"
                      onClick={() => scopes_remove(index)}
                      className={cn(
                        "text-red-500 hover:text-red-700 sm:self-center",
                        index === 0 && "sm:mt-14"
                      )}
                    >
                      <MinusCircle className="h-5 w-5" />
                    </Button>
                  </div>
                ))}
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  className="mt-2"
                  onClick={() => scopes_append({ value: "" })}
                >
                  <Plus className="mr-2 h-4 w-4" /> Add Scope
                </Button>
              </div>
              <div>
                {params.map((field, index) => (
                  <div className="flex flex-col gap-4 sm:flex-row sm:items-center" key={field.id + index}>
                    <div className="flex flex-1 gap-4">
                      <FormField
                        control={form.control}
                        name={`extra_params.${index}.key`}
                        render={({ field }) => (
                          <FormItem className="flex-1">
                            <FormLabel className={cn(index !== 0 && "sr-only")}>Key:</FormLabel>
                            <FormDescription className={cn(index !== 0 && "sr-only")}>
                              Enter the key here.
                            </FormDescription>
                            <FormControl>
                              <Input {...field} />
                            </FormControl>
                            <FormMessage />
                          </FormItem>
                        )}
                      />
                      <FormField
                        control={form.control}
                        name={`extra_params.${index}.value`}
                        render={({ field }) => (
                          <FormItem className="flex-1">
                            <FormLabel className={cn(index !== 0 && "sr-only")}>Value:</FormLabel>
                            <FormDescription className={cn(index !== 0 && "sr-only")}>
                              Enter the value here.
                            </FormDescription>
                            <FormControl>
                              <Input {...field} />
                            </FormControl>
                            <FormMessage />
                          </FormItem>
                        )}
                      />
                    </div>
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon"
                      onClick={() => params_remove(index)}
                      className={cn(
                        "text-red-500 hover:text-red-700 sm:self-center",
                        index === 0 && "sm:mt-14"
                      )}
                    >
                      <MinusCircle className="h-5 w-5" />
                    </Button>
                  </div>
                ))}
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  className="mt-2"
                  onClick={() => params_append({ key: "", value: "" })}
                >
                  <Plus className="mr-2 h-4 w-4" /> Add Extra Params
                </Button>
              </div>
              <FormField
                control={form.control}
                name='use_proxy'
                render={({ field }) => (
                  <FormItem>
                    <FormLabel className="flex items-center justify-between">Use Proxy (optional):</FormLabel>
                    <FormControl>
                      <Select
                        onValueChange={(val) => field.onChange(Number(val))}
                        defaultValue={field.value?.toString()}
                      >
                        <FormControl>
                          <SelectTrigger>
                            <SelectValue placeholder="Select a proxy" />
                          </SelectTrigger>
                        </FormControl>
                        <SelectContent>
                          {proxyOptions && proxyOptions.length > 0 ? (
                            proxyOptions.map((option) => (
                              <SelectItem key={option.value} value={option.value.toString()}>
                                {option.label}
                              </SelectItem>
                            ))
                          ) : (
                            <SelectItem disabled value="__none__">No proxy available</SelectItem>
                          )}
                        </SelectContent>
                      </Select>
                    </FormControl>
                    <FormDescription className='flex-1'>
                      Use SOCKS5 proxy for OAuth requests when direct access is blocked.
                    </FormDescription>
                    <FormMessage />
                  </FormItem>
                )}
              />
              <FormField
                control={form.control}
                name='description'
                render={({ field }) => (
                  <FormItem className='flex flex-col gap-y-1 space-y-0'>
                    <FormLabel className='mb-1'>Description:</FormLabel>
                    <FormControl>
                      <Textarea
                        placeholder='Describe the purpose of the oauth2 application'
                        {...field}
                        className="max-h-[240px] min-h-[80px]"
                      />
                    </FormControl>
                    <FormDescription>(Optional)</FormDescription>
                    <FormMessage />
                  </FormItem>
                )}
              />
            </form>
          </Form>
        </ScrollArea>
        <DialogFooter>
          <Button
            type="submit"
            form="oauth2-form"
            disabled={isEdit ? updateMutation.isPending : createMutation.isPending}
            className="relative"
          >
            {isEdit ? (
              updateMutation.isPending ? (
                <span className="flex items-center justify-center">
                  <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                  Saving...
                </span>
              ) : (
                "Save changes"
              )
            ) : (
              createMutation.isPending ? (
                <span className="flex items-center justify-center">
                  <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                  Creating...
                </span>
              ) : (
                "Save"
              )
            )}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

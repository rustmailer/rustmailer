/*
 * Copyright © 2025-2026 rustmailer.com
 * Licensed under RustMailer License Agreement v1.0
 * Unauthorized use or distribution is prohibited.
 */

import { Button } from '@/components/ui/button'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import { Attachment, EmailBodyPart, EmailEnvelope, formatAddressList } from '../data/schema'
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@/components/ui/tooltip'
import { Download, FileCode, FileCode2, Forward, Loader, MailCheck, MailQuestion, MoreVertical, Reply, ReplyAll, Trash2 } from 'lucide-react'
import { Separator } from '@/components/ui/separator'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { useEffect, useState } from 'react'
import { AttachmentInfo, download_attachment, get_full_message, getContent, load_message } from '@/api/mailbox/envelope/api'
import { useMutation } from '@tanstack/react-query'
import { toast } from '@/hooks/use-toast'
import { formatFileSize } from '@/lib/utils'
import EmailIframe from './mail-iframe'
import { MailboxDialogType } from '../context'
import { useFlagMessageMutation } from '@/hooks/use-flag-messages'
import { ScrollArea } from '@/components/ui/scroll-area'
import { MailboxData } from '@/api/mailbox/api'
import { AxiosError } from 'axios'
import { EmailAction, EmailActionDialog } from './email-action-dialog'
import { useMoveMessageMutation } from '@/hooks/use-move-messages'
import { useCopyMessageMutation } from '@/hooks/use-copy-messages'

interface MultilinesProps {
  title: string,
  lines: string[];
}

const Multilines: React.FC<MultilinesProps> = ({ lines, title }) => {
  const [expanded, setExpanded] = useState(false);

  return (
    <div className="text-xs">
      <div className="flex items-start space-x-2">
        <span className="font-medium text-gray-400 whitespace-nowrap">{title}:</span>
        <div className="flex-1">
          <ul className="list-disc list-inside">
            {lines.slice(0, expanded ? lines.length : 3).map((ref, index) => (
              <li key={index} className="line-clamp-1">{ref}</li>
            ))}
          </ul>
          {lines.length > 3 && (
            <button
              className="text-blue-500 hover:underline"
              onClick={() => setExpanded(!expanded)}
            >
              {expanded ? 'show less' : 'show more...'}
            </button>
          )}
        </div>
      </div>
    </div>
  );
};


interface Props {
  open: boolean
  onOpenChange: (open: boolean) => void
  currentEnvelope?: EmailEnvelope | undefined
  currentMailbox?: MailboxData | undefined
  currentAccountId?: number | undefined
  setDeleteIds: React.Dispatch<React.SetStateAction<string[]>>;
  setOpen: (str: MailboxDialogType | null) => void
}

export function MailDisplayDrawer({ open, setOpen, onOpenChange, currentEnvelope, setDeleteIds, currentMailbox, currentAccountId }: Props) {
  const [downloadingAttachmentId, setDownloadingAttachmentId] = useState<string | null>(null);
  const [content, setContent] = useState<string | null>(null);

  const [gmailOrGraphAttachments, setGmailOrGraphAttachments] = useState<AttachmentInfo[] | null>(null);

  const [loading, setLoading] = useState(false);
  const [contentType, setContentType] = useState<'Plain' | 'Html' | null>(null);

  const [dialogOpen, setDialogOpen] = useState(false);
  const [dialogAction, setDialogAction] = useState<EmailAction>('reply');

  const { mutate: flagMessage } = useFlagMessageMutation();
  const { mutate: moveMessage } = useMoveMessageMutation();
  const { mutate: copyMessage } = useCopyMessageMutation();

  const isGmailApi = currentEnvelope && isNaN(Number(currentEnvelope.id));

  const downloadMutation = useMutation({
    mutationFn: ({ accountId, fileName, payload }: { accountId: number, fileName: string | undefined, payload: Record<string, any> }) => download_attachment(accountId, fileName, payload),
    retry: false,
    onSuccess: () => {
      setDownloadingAttachmentId(null);
    },
    onError: (error) => {
      setDownloadingAttachmentId(null);
      toast({
        title: 'Failed to download file',
        description: `${error.message}`,
        variant: 'destructive'
      })
    },
  });

  const loadMessageMutation = useMutation({
    mutationFn: ({ accountId, payload }: { accountId: number, payload?: Record<string, any> }) => {
      if (payload) {
        return load_message(accountId, payload)
      } else {
        throw new Error("Payload must be provided")
      }
    },
    retry: 0,
    onSuccess: (data) => {
      setLoading(false)
      setContent(getContent(data))
      if (data.attachments) {
        setGmailOrGraphAttachments(data.attachments)
      }
      if (data.html) {
        setContentType('Html');
      } else {
        setContentType('Plain');
      }
    },
    onError: (error) => {
      setLoading(false)
      const isDisabled = (error as any)?.response?.status === 403;
      toast({
        title: isDisabled ? 'Account is disabled' : 'Failed to load email message.',
        description: isDisabled
          ? 'This account is disabled. Message content is unavailable.'
          : `${error.message}`,
        variant: 'destructive'
      })
      if (isDisabled) {
        setOpen(null);
      }
    },
  });

  const onDownload = (attachment: Attachment) => {
    if (currentEnvelope) {

      let payload = {
        id: currentEnvelope.id,
        mailbox: currentMailbox?.name,
        attachment
      };
      setDownloadingAttachmentId(attachment.id);
      downloadMutation.mutate({ accountId: currentAccountId!, fileName: attachment.filename, payload })
    }
  }


  const onGmailOrGraphAttachmentDownload = (attachment_info: AttachmentInfo) => {
    if (currentEnvelope) {
      let payload = {
        attachment_info,
        filename: attachment_info.filename,
        id: currentEnvelope.id,
        attachment_id: attachment_info.id
      };
      setDownloadingAttachmentId(attachment_info.id);
      downloadMutation.mutate({ accountId: currentAccountId!, fileName: attachment_info.filename, payload })
    }
  }

  useEffect(() => {
    if (open && currentEnvelope) {
      if (isGmailApi) {
        setLoading(true);
        let payload: {
          id: string;
        } = {
          id: currentEnvelope.id
        };
        loadMessageMutation.mutate({ accountId: currentAccountId!, payload })
      } else {
        const emailBodyParts = currentEnvelope.body_meta || [];
        const htmlPart = emailBodyParts.find(part => part.part_type === 'Html');
        const textPart = emailBodyParts.find(part => part.part_type === 'Plain');
        setLoading(true);
        if (htmlPart) {
          onLoadMessage(htmlPart);
          // setContentType('Html');
        } else if (textPart) {
          onLoadMessage(textPart);
          // setContentType('Plain');
        }
      }
    }
  }, [currentEnvelope, open]);

  const onLoadMessage = async (emailbody: EmailBodyPart) => {
    if (currentEnvelope) {
      let inlineAttachments: Attachment[] = [];
      if (currentEnvelope.attachments) {
        inlineAttachments = currentEnvelope.attachments.filter(attachment => attachment.inline === true);
      }
      let payload: {
        id: string;
        mailbox: string | undefined;
        sections: EmailBodyPart[];
        inline?: Attachment[]; // Declare inline as an optional field
      } = {
        id: currentEnvelope.id,
        mailbox: currentMailbox?.name,
        sections: [emailbody],
      };

      if (inlineAttachments.length > 0) {
        payload.inline = inlineAttachments;
      }
      loadMessageMutation.mutate({ accountId: currentAccountId!, payload })
    }
  }

  const onMarkAsRead = () => {
    if (currentEnvelope) {
      if (isGmailApi) {
        let payload = {
          ids: [currentEnvelope.id],
          current_mailbox: "UNREAD",
          target_mailbox: currentMailbox?.name
        };
        moveMessage({ accountId: currentAccountId!, payload })
      } else {
        let payload = {
          uids: [Number(currentEnvelope.id)],
          mailbox: currentMailbox?.name,
          action: {
            add: [{ flag: "Seen" }]
          }
        };
        flagMessage({ accountId: currentAccountId!, payload })
      }
    }
  }

  const onMarkAsUnread = () => {
    if (currentEnvelope) {
      if (isGmailApi) {
        let payload = {
          ids: [currentEnvelope.id],
          current_mailbox: currentMailbox?.name,
          target_mailbox: "UNREAD"
        };
        copyMessage({ accountId: currentAccountId!, payload })
      } else {
        let payload = {
          uids: [Number(currentEnvelope.id)],
          mailbox: currentMailbox?.name,
          action: {
            remove: [{ flag: "Seen" }]
          }
        };
        flagMessage({ accountId: currentAccountId!, payload })
      }
    }
  }

  const handleDelete = () => {
    if (currentEnvelope) {
      setDeleteIds([currentEnvelope.id])
      setOpen('move-to-trash')
    }
  }

  const downloadHtmlFile = () => {
    if (loading) {
      toast({
        title: 'Download unavailable',
        description: 'Content is still being loaded. Please wait.'
      });
      return;
    }

    if (!content) {
      toast({
        title: 'Download failed',
        description: 'No content available to download.'
      });
      return;
    }

    try {
      const fileName = currentEnvelope?.subject || 'email_message';
      const formattedFileName = fileName.endsWith('.html')
        ? fileName
        : `${fileName}.html`;

      const blob = new Blob([content], { type: 'text/html' });
      const url = URL.createObjectURL(blob);

      const a = document.createElement('a');
      a.href = url;
      a.download = formattedFileName;
      document.body.appendChild(a);
      a.click();

      setTimeout(() => {
        document.body.removeChild(a);
        URL.revokeObjectURL(url);
      }, 100);

      toast({
        title: 'Download started',
        description: `"${formattedFileName}" is being downloaded`,
      });

    } catch (error) {
      toast({
        title: 'Download failed',
        description: error instanceof Error ? error.message : 'An unknown error occurred',
        variant: 'destructive',
      });
    }
  };

  async function downloadEmlFile() {
    const filename = currentEnvelope?.subject || 'email_message';
    if (!currentAccountId || !currentEnvelope?.mailbox_name || !currentEnvelope) {
      toast({
        title: 'Download failed',
        description: 'Cannot download - missing required message information',
        variant: 'destructive',
      });
      return; // Early return if required data is missing
    }

    try {
      toast({
        title: 'Download started',
        description: `"${filename}" is being downloaded`,
      });

      // Fetch the message
      await get_full_message(currentAccountId!, currentEnvelope?.mailbox_name, currentEnvelope?.id, filename);
      // Optional: Show download complete notification
      toast({
        title: 'Download complete',
        description: `"${filename}" has been downloaded successfully`,
      });
    } catch (error) {
      // Show error notification
      let errorMessage = 'Failed to download email';

      // Handle AxiosError specifically
      if (error instanceof AxiosError) {
        // Try to get server response message first
        errorMessage = error.response?.data?.message
          || error.response?.data?.error
          || error.message;

        // Include status code if available
        if (error.response?.status) {
          errorMessage = `${error.response.status}: ${errorMessage}`;
        }
      }
      // Fallback for non-Axios errors
      else if (error instanceof Error) {
        errorMessage = error.message;
      }

      toast({
        title: 'Download failed',
        description: `Failed to download "${filename}": ${errorMessage}`,
        variant: 'destructive',
      });

      console.error('Download error:', error);
    }
  }


  return (
    <Sheet
      open={open}
      onOpenChange={(open) => {
        setLoading(false)
        onOpenChange(open)
        setContent(null)
      }}
    >
      <SheetContent className='md:w-[60rem] h-full'>
        <SheetHeader className='text-left mt-4'>
          <SheetTitle>Email Details</SheetTitle>
        </SheetHeader>
        <SheetDescription></SheetDescription>
        <div className="flex items-center">
          <div className="flex items-center gap-2">
            <Tooltip>
              <TooltipTrigger asChild>
                <Button variant="ghost" size="icon" onClick={onMarkAsRead}>
                  <MailCheck className="h-4 w-4" />
                  <span className="sr-only">Mark as read</span>
                </Button>
              </TooltipTrigger>
              <TooltipContent>Mark as read</TooltipContent>
            </Tooltip>
            <Tooltip>
              <TooltipTrigger asChild>
                <Button variant="ghost" size="icon" onClick={onMarkAsUnread}>
                  <MailQuestion className="h-4 w-4" />
                  <span className="sr-only">Mark as unread</span>
                </Button>
              </TooltipTrigger>
              <TooltipContent>Mark as unread</TooltipContent>
            </Tooltip>
            <Tooltip>
              <TooltipTrigger asChild>
                <Button variant="ghost" size="icon" onClick={handleDelete}>
                  <Trash2 className="h-4 w-4" />
                  <span className="sr-only">Move to trash</span>
                </Button>
              </TooltipTrigger>
              <TooltipContent>Move to trash</TooltipContent>
            </Tooltip>
          </div>
          <div className="ml-auto flex items-center gap-2">
            <Tooltip>
              <TooltipTrigger asChild>
                <Button variant="ghost" size="icon" onClick={() => {
                  setDialogAction("reply");
                  setDialogOpen(true);
                }}>
                  <Reply className="h-4 w-4" />
                  <span className="sr-only">Reply</span>
                </Button>
              </TooltipTrigger>
              <TooltipContent>Reply</TooltipContent>
            </Tooltip>
            <Tooltip>
              <TooltipTrigger asChild>
                <Button variant="ghost" size="icon" onClick={() => {
                  setDialogAction("replyAll");
                  setDialogOpen(true);
                }}>
                  <ReplyAll className="h-4 w-4" />
                  <span className="sr-only">Reply all</span>
                </Button>
              </TooltipTrigger>
              <TooltipContent>Reply all</TooltipContent>
            </Tooltip>
            <Tooltip>
              <TooltipTrigger asChild>
                <Button variant="ghost" size="icon" onClick={() => {
                  setDialogAction("forward");
                  setDialogOpen(true);
                }}>
                  <Forward className="h-4 w-4" />
                  <span className="sr-only">Forward</span>
                </Button>
              </TooltipTrigger>
              <TooltipContent>Forward</TooltipContent>
            </Tooltip>
          </div>
          <Separator orientation="vertical" className="mx-2 h-6" />
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button variant="ghost" size="icon">
                <MoreVertical className="h-4 w-4" />
                <span className="sr-only">Download options</span>
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              <DropdownMenuItem onClick={downloadEmlFile}>
                <FileCode2 className="mr-2 h-4 w-4" />
                <span className='text-xs'>EML</span>
              </DropdownMenuItem>
              <DropdownMenuItem onClick={downloadHtmlFile}>
                <FileCode className="mr-2 h-4 w-4" />
                <span className='text-xs'>HTML</span>
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
        <Separator />
        <ScrollArea className="max-h-full w-full pr-4 -mr-4 py-1">
          <div className="flex flex-col">
            {currentEnvelope ? (
              <div className="flex flex-1 flex-col">
                <div className="flex items-start">
                  <div className="grid gap-1">
                    <div className="truncate text-xs">
                      <span className="font-medium text-gray-400">ID:</span>
                      <span className="ml-2">{currentEnvelope.id}</span>
                    </div>
                    {currentEnvelope.from && (
                      <div className="line-clamp-1 text-xs space-x-2">
                        <span className="font-medium text-gray-400">From:</span>
                        <span>{currentEnvelope.from?.address}</span>
                      </div>
                    )}
                    {currentEnvelope.to && currentEnvelope.to.length > 0 && (
                      <div className="line-clamp-1 text-xs space-x-2">
                        <Multilines title='To' lines={formatAddressList(currentEnvelope.to)} />
                      </div>
                    )}
                    {currentEnvelope.reply_to && currentEnvelope.reply_to.length > 0 && (
                      <div className="line-clamp-1 text-xs space-x-2">
                        <Multilines title='Reply To' lines={formatAddressList(currentEnvelope.reply_to)} />
                      </div>
                    )}
                    {currentEnvelope.cc && currentEnvelope.cc.length > 0 && (
                      <div className="line-clamp-1 text-xs space-x-2">
                        <Multilines title='Cc' lines={formatAddressList(currentEnvelope.cc)} />
                      </div>
                    )}
                    {currentEnvelope.bcc && currentEnvelope.bcc.length > 0 && (
                      <div className="line-clamp-1 text-xs space-x-2">
                        <Multilines title='Bcc' lines={formatAddressList(currentEnvelope.bcc)} />
                      </div>
                    )}
                    {currentEnvelope.in_reply_to && (
                      <div className="line-clamp-1 text-xs space-x-2">
                        <span className="font-medium text-gray-400">In-Reply-To:</span>
                        <span>{currentEnvelope.in_reply_to}</span>
                      </div>
                    )}
                    {currentEnvelope.subject && (
                      <div className="line-clamp-1 text-xs space-x-2">
                        <span className="font-medium text-gray-400">Subject:</span>
                        <span>{currentEnvelope.subject}</span>
                      </div>
                    )}
                    {currentEnvelope.message_id && (
                      <div className="line-clamp-1 text-xs space-x-2">
                        <span className="font-medium text-gray-400">Message-Id:</span>
                        <span>{currentEnvelope.message_id}</span>
                      </div>
                    )}
                    {currentEnvelope.references && (
                      <div className="line-clamp-1 text-xs space-x-2">
                        <Multilines title='References' lines={currentEnvelope.references} />
                      </div>
                    )}
                    {currentEnvelope.internal_date && (
                      <div className="line-clamp-1 text-xs space-x-2">
                        <span className="font-medium text-gray-400">Internal-Date:</span>
                        <span>{formatTimestamp(currentEnvelope.internal_date)}</span>
                      </div>
                    )}
                    {currentEnvelope.date && (
                      <div className="line-clamp-1 text-xs space-x-2">
                        <span className="font-medium text-gray-400">Date:</span>
                        <span>{formatTimestamp(currentEnvelope.date)}</span>
                      </div>
                    )}
                  </div>
                </div>
                <Separator />
                <div className="whitespace-pre-wrap mt-2 mb-2">
                  {currentEnvelope.attachments && currentEnvelope.attachments.length > 0 ? (
                    <div className="space-y-2">
                      {currentEnvelope.attachments.map((attachment, index) => (
                        <div key={index} className="flex items-center">
                          <div className="flex items-center space-x-8">
                            <span className="truncate text-xs">{attachment.filename}</span>
                            <span className="text-xs px-2 py-1 rounded">
                              [{attachment.file_type}]
                            </span>
                            {attachment.inline && (
                              <span className="text-xs text-blue-500 bg-blue-100 px-2 py-1 rounded">
                                Inline
                              </span>
                            )}
                          </div>
                          <div className="flex items-center space-x-4 ml-auto">
                            <span className="text-gray-500 text-xs shrink-0">
                              {formatFileSize(attachment.size)}
                            </span>
                            <TooltipProvider>
                              <Tooltip>
                                <TooltipTrigger asChild>
                                  {downloadingAttachmentId === attachment.id ? <Loader className='w-4 h-4 mb-1 animate-spin' /> : <Download className='w-4 h-4 mb-1' onClick={() => onDownload(attachment)} />}
                                </TooltipTrigger>
                                <TooltipContent>
                                  <p className='text-xs'>Download</p>
                                </TooltipContent>
                              </Tooltip>
                            </TooltipProvider>
                          </div>
                        </div>
                      ))}
                    </div>
                  ) : loading ? (<span className="text-gray-500 text-xs min-h-3" />) : gmailOrGraphAttachments && gmailOrGraphAttachments.length > 0 ? (<div className="space-y-2">
                    {gmailOrGraphAttachments.map((attachment, index) => (
                      <div key={index} className="flex items-center">
                        <div className="flex items-center space-x-8">
                          <span className="truncate text-xs">{attachment.filename}</span>
                          <span className="text-xs px-2 py-1 rounded">
                            [{attachment.file_type}]
                          </span>
                          {attachment.inline && (
                            <span className="text-xs text-blue-500 bg-blue-100 px-2 py-1 rounded">
                              Inline
                            </span>
                          )}
                        </div>
                        <div className="flex items-center space-x-4 ml-auto">
                          <span className="text-gray-500 text-xs shrink-0">
                            {formatFileSize(attachment.size)}
                          </span>
                          <TooltipProvider>
                            <Tooltip>
                              <TooltipTrigger asChild>
                                {downloadingAttachmentId === attachment.id ? <Loader className='w-4 h-4 mb-1 animate-spin' /> : <Download className='w-4 h-4 mb-1' onClick={() => onGmailOrGraphAttachmentDownload(attachment)} />}
                              </TooltipTrigger>
                              <TooltipContent>
                                <p className='text-xs'>Download</p>
                              </TooltipContent>
                            </Tooltip>
                          </TooltipProvider>
                        </div>
                      </div>
                    ))}
                  </div>) : (
                    <span className="text-gray-500 text-xs">No attachments</span>
                  )}
                </div>
                <Separator />
                <div className=''>
                  {loading ? (
                    <div className="flex justify-center items-center">
                      <Loader className="w-6 h-6 animate-spin" />
                      <span className="ml-2 text-sm text-muted-foreground">loading...</span>
                    </div>
                  ) : content ? (
                    <div className="bg-gray-100 rounded-lg border-gray-300">
                      {contentType === "Html" ? (
                        // If contentType is "html", render the content as HTML
                        <EmailIframe emailHtml={content} />
                      ) : (
                        // If contentType is "text", render the content as plain text
                        <pre className="whitespace-pre-wrap text-gray-800 text-left text-sm">
                          {content}
                        </pre>
                      )}
                    </div>
                  ) : (
                    <div className="text-center text-muted-foreground text-sm">No content available</div>
                  )}
                </div>
              </div>
            ) : (
              <div className="p-8 text-center text-muted-foreground">
                No message selected
              </div>
            )}
          </div>
        </ScrollArea>
        <EmailActionDialog open={dialogOpen} onOpenChange={setDialogOpen} currentEnvelope={currentEnvelope} currentAccountId={currentAccountId} action={dialogAction} />
      </SheetContent>
    </Sheet>)
}


function formatTimestamp(milliseconds: number): string {
  const date = new Date(milliseconds);
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, '0');
  const day = String(date.getDate()).padStart(2, '0');
  const hours = String(date.getHours()).padStart(2, '0');
  const minutes = String(date.getMinutes()).padStart(2, '0');
  const seconds = String(date.getSeconds()).padStart(2, '0');

  const timezoneOffset = date.getTimezoneOffset();
  const offsetSign = timezoneOffset > 0 ? '-' : '+';
  const offsetHours = String(Math.floor(Math.abs(timezoneOffset) / 60)).padStart(2, '0');
  const offsetMinutes = String(Math.abs(timezoneOffset) % 60).padStart(2, '0');

  return `${year}-${month}-${day}T${hours}:${minutes}:${seconds}${offsetSign}${offsetHours}:${offsetMinutes}`;
}

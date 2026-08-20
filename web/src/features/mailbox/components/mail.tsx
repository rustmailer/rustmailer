/*
 * Copyright © 2025-2026 rustmailer.com
 * Licensed under RustMailer License Agreement v1.0
 * Unauthorized use or distribution is prohibited.
 */

import * as React from "react"
import { cn } from "@/lib/utils"
import {
    ResizableHandle,
    ResizablePanel,
    ResizablePanelGroup,
} from "@/components/ui/resizable"
import { Separator } from "@/components/ui/separator"
import { TooltipProvider } from "@/components/ui/tooltip"
import { AccountSwitcher } from "./account-switcher"
import { TreeView } from "@/components/tree-view"
import { ScrollArea } from "@/components/ui/scroll-area"
import { list_account_mailboxes, MailboxData } from "@/api/mailbox/api"
import { useQuery, useQueryClient } from "@tanstack/react-query"
import { buildTree } from "../../../lib/build-tree"
import { Skeleton } from "@/components/ui/skeleton"
import { EmailEnvelope } from "../data/schema"
import MailboxProvider, { MailboxDialogType } from "../context"
import useDialogState from "@/hooks/use-dialog-state"
import { MailboxDialog } from "./mailbox-detail"
import { MailList } from "./mail-list"
import { EnvelopeListPagination } from "./pagination"
import { list_messages, search_messages } from "@/api/mailbox/envelope/api"
import { MailDisplayDrawer } from "./mail-display-drawer"
import { toast } from "@/hooks/use-toast"
import { Switch } from "@/components/ui/switch"
import { Dot, Flag, ListFilterPlus, MailIcon, MailOpen, Trash2, X } from "lucide-react"
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover"
import { Button } from "@/components/ui/button"
import { MoveTo } from "./move-to"
import { CustomFlagInput } from "./custom-flag"
import { FilterForm } from "./envelope-filter"
import { EnvelopeDeleteDialog } from "./delete-dialog"
import { useFlagMessageMutation } from "@/hooks/use-flag-messages"
import { EnvelopeFilterDialog } from "./envelope-filter-dialog"
import Logo from '@/assets/logo.svg'
import { PaginatedResponse } from "@/api"
import { useMoveMessageMutation } from "@/hooks/use-move-messages"
import { useCopyMessageMutation } from "@/hooks/use-copy-messages"

interface MailProps {
    defaultLayout: number[] | undefined
    defaultCollapsed?: boolean
    navCollapsedSize: number,
    lastSelectedAccountId?: number | undefined
}

interface ListMessagesOptions {
    accountId: number | undefined;
    mailbox: string | undefined;
    next_page_token: string | undefined;
    page_size: number;
    remote: boolean;
    filter?: FilterForm;
}


function buildPayload(filterForm: FilterForm): any {
    if (!filterForm.operator) {

        if (!filterForm.conditions || filterForm.conditions.length !== 1) {
            throw new Error("If operator is empty, there must be exactly one condition.");
        }

        const condition = filterForm.conditions[0];
        if (condition.operator === "is not") {
            return {
                type: "Logic",
                operator: "Not",
                children: [
                    {
                        type: "Condition",
                        condition: condition.field,
                        value: condition.value
                    }
                ]
            };
        }

        return {
            type: "Condition",
            condition: condition.field,
            value: condition.value
        };
    }

    return {
        type: "Logic",
        operator: filterForm.operator.toUpperCase(),
        children: filterForm.conditions?.map(condition => {
            if (condition.operator === "is not") {
                return {
                    type: "Logic",
                    operator: "Not",
                    children: [
                        {
                            type: "Condition",
                            condition: condition.field,
                            value: condition.value
                        }
                    ]
                };
            }

            return ({
                type: "Condition",
                condition: condition.field,
                value: condition.value
            })
        }) || []
    };
}

// const useListMessages = ({ accountId, mailbox, page, page_size, remote, filter }: ListMessagesOptions) => {
//     return useQuery({
//         queryKey: ['mailbox-list-messages', `${accountId}`, mailbox, page, page_size, remote, filter],
//         queryFn: () => {
//             if (filter) {
//                 const payload = {
//                     mailbox: mailbox!,
//                     search: buildPayload(filter)
//                 };
//                 return search_messages(accountId!, page, page_size, remote, payload);
//             }
//             return list_messages(accountId!, mailbox!, page, page_size, remote);
//         },
//         enabled: !!accountId && !!mailbox,
//     });
// };


export async function listMessagesAPI({
    accountId,
    mailbox,
    next_page_token,
    page_size,
    remote,
    filter
}: ListMessagesOptions) {
    if (filter) {
        const payload = {
            mailbox,
            search: buildPayload(filter)
        };
        return await search_messages(accountId!, page_size, remote, payload, next_page_token);
    }

    return await list_messages(accountId!, mailbox!, page_size, remote, next_page_token);
}



export function Mail({
    defaultLayout = [20, 80],
    defaultCollapsed = false,
    navCollapsedSize,
    lastSelectedAccountId,
}: MailProps) {
    const [open, setOpen] = useDialogState<MailboxDialogType>(null)
    const [isCollapsed, setIsCollapsed] = React.useState(defaultCollapsed)
    const [selectedMailbox, setSelectedMailbox] = React.useState<MailboxData | undefined>(undefined);
    const [selectedAccountId, setSelectedAccountId] = React.useState<number | undefined>(lastSelectedAccountId);
    const [selectedEvelope, setSelectedEvelope] = React.useState<EmailEnvelope | undefined>(undefined);
    const [remote, setRemote] = React.useState<boolean>(false);

    const [envelopes, setEnvelopes] = React.useState<PaginatedResponse<EmailEnvelope> | undefined>(undefined);
    const [isMessagesLoading, setIsMessagesLoading] = React.useState<boolean>(false);
    const [isError, setIsError] = React.useState<boolean>(false);
    const [error, setError] = React.useState<any>(undefined);
    // const [pageTokenMap, setPageTokenMap] = React.useState<Record<number, string | undefined>>({});
    const pageTokenMapRef = React.useRef<Record<number, string | undefined>>({});

    const [page, setPage] = React.useState(0);
    const [pageSize, setPageSize] = React.useState(30);
    const [selectedIds, setSelectedIds] = React.useState<string[]>([]);
    const [deleteIds, setDeleteIds] = React.useState<string[]>([]);
    const [currentFilter, setCurrentFilter] = React.useState<FilterForm | undefined>(undefined);
    const [isSearching, setIsSearching] = React.useState(false);
    // const [customFlags, setCustomFlags] = React.useState<string[]>([]);
    const { mutate: flagMessage } = useFlagMessageMutation();
    const { mutate: moveMessage } = useMoveMessageMutation();
    const { mutate: copyMessage } = useCopyMessageMutation();

    const queryClient = useQueryClient();

    const { data: mailboxes, isLoading: isMailboxesLoading } = useQuery({
        queryKey: ['account-mailboxes', `${selectedAccountId}`, remote],
        queryFn: () => list_account_mailboxes(selectedAccountId!, remote),
        enabled: !!selectedAccountId,
    })


    const isGmailApi = envelopes?.items.some(item => {
        if (!item.id) return false;
        return isNaN(Number(item.id));
    });

    React.useEffect(() => {
        if (!selectedAccountId || !selectedMailbox) {
            return;
        }

        (async () => {
            setIsMessagesLoading(true);
            setIsError(false);
            setError(undefined);
            const next_page_token = pageTokenMapRef.current[page];
            try {
                const data = await listMessagesAPI({
                    accountId: selectedAccountId,
                    mailbox: selectedMailbox?.name,
                    page_size: pageSize,
                    remote: remote,
                    filter: currentFilter,
                    next_page_token
                });
                setEnvelopes(data);
                pageTokenMapRef.current = {
                    ...pageTokenMapRef.current,
                    [page + 1]: data.next_page_token ?? undefined,
                };
            } catch (error) {
                setIsError(true);
                setError(error);
                // Stop retrying: clear the selected mailbox so dependency changes
                // (page, remote, filter) do not re-trigger this effect in a loop.
                setSelectedMailbox(undefined);
            } finally {
                setIsMessagesLoading(false)
            }
        })();
    }, [selectedAccountId, selectedMailbox, page, pageSize, remote, currentFilter])

    // const { data: envelopes, isLoading: isMessagesLoading, isError, error } = useListMessages({
    //     accountId: selectedAccountId,
    //     mailbox: selectedMailbox?.name,
    //     page: page + 1,
    //     page_size: pageSize,
    //     remote: useIMAP,
    //     filter: currentFilter
    // });

    const triggerUpdate = (mailbox: string) => {
        queryClient.refetchQueries({
            queryKey: ['mailbox-list-messages', selectedAccountId, mailbox, page + 1, pageSize, remote, currentFilter]
        });
    }

    const handleSearch = async (data: FilterForm) => {
        if (selectedAccountId && selectedMailbox) {
            setIsSearching(true);
            try {
                setPage(0);
                setPageSize(10);
                setSelectedIds([]);
                setCurrentFilter(data);

                const payload = {
                    mailbox: selectedMailbox?.name,
                    search: buildPayload(data)
                };
                const result = await search_messages(selectedAccountId, 10, remote, payload);
                setEnvelopes(result);
            } catch (error) {
                console.error('Error fetching messages:', error);
                toast({
                    variant: "destructive",
                    title: "Failed to search messages",
                    description: "An error occurred while applying filters",
                });
            } finally {
                setIsSearching(false);
            }
        }
    }

    const clearFilter = () => {
        setCurrentFilter(undefined);
        setPage(0);
        setPageSize(10);
        setSelectedIds([]);
    }

    const hasNextPage = () => {
        return !!pageTokenMapRef.current[page + 1];
    }

    const handlePageChange = (newPage: number) => {
        setPage(newPage);
        setSelectedIds([]);
    }


    const handlePageSizeChange = (newSize: number) => {
        setPage(0);
        setPageSize(newSize);
        setSelectedIds([]);
    }

    const handleMarkFolderRead = () => {
        if (selectedAccountId && selectedMailbox) {
            if (isGmailApi) {
                let payload = {
                    ids: selectedIds,
                    current_mailbox: "UNREAD",
                    target_mailbox: selectedMailbox?.name
                };
                moveMessage({ accountId: selectedAccountId, payload })
            } else {
                let payload = {
                    uids: selectedIds.map(id => Number(id)),
                    mailbox: selectedMailbox?.name,
                    action: {
                        add: [{ flag: "Seen" }]
                    }
                };
                flagMessage({ accountId: selectedAccountId, payload })
            }
        }
    }

    const handleMarkFolderUnread = () => {
        if (selectedAccountId && selectedMailbox) {
            if (isGmailApi) {
                let payload = {
                    ids: selectedIds,
                    current_mailbox: selectedMailbox?.name,
                    target_mailbox: "UNREAD"
                };
                copyMessage({ accountId: selectedAccountId, payload })
            } else {
                let payload = {
                    uids: selectedIds.map(id => Number(id)),
                    mailbox: selectedMailbox?.name,
                    action: {
                        remove: [{ flag: "Seen" }]
                    }
                };
                flagMessage({ accountId: selectedAccountId, payload })
            }
        }
    }

    React.useEffect(() => {
        if (isError && error) {
            const isDisabled =
                error?.response?.status === 403 ||
                error?.message?.includes('disabled');
            toast({
                variant: "destructive",
                title: isDisabled ? "Account is disabled" : "Failed to load messages",
                description: isDisabled
                    ? "This account is disabled. Only cached messages are available."
                    : (error.message || "An unknown error occurred. Please try again."),
            });
        }
    }, [isError, error]);

    return (
        <MailboxProvider value={{ open, setOpen, currentMailbox: selectedMailbox, setCurrentMailbox: setSelectedMailbox, currentEnvelope: selectedEvelope, setCurrentEnvelope: setSelectedEvelope, deleteIds, setDeleteIds }}>
            <TooltipProvider delayDuration={0}>
                <ResizablePanelGroup
                    direction="horizontal"
                    onLayout={(sizes: number[]) => {
                        localStorage.setItem('react-resizable-panels:layout:mail', JSON.stringify(sizes));
                    }}
                    className="items-stretch"
                >
                    <ResizablePanel
                        defaultSize={defaultLayout[0]}
                        collapsedSize={navCollapsedSize}
                        minSize={navCollapsedSize}
                        collapsible={true}
                        onCollapse={() => {
                            setIsCollapsed(true);
                            localStorage.setItem('react-resizable-panels:collapsed', JSON.stringify(true));
                        }}
                        onResize={() => {
                            setIsCollapsed(false);
                            localStorage.setItem('react-resizable-panels:collapsed', JSON.stringify(false));
                        }}
                        className={cn(
                            isCollapsed &&
                            "min-w-[50px] transition-all duration-300 ease-in-out"
                        )}
                    >
                        <Separator className="mb-2" />
                        <ScrollArea className='h-[50rem] w-full pr-4 -mr-4 py-1'>
                            <div className="flex flex-row items-center justify-between rounded-lg border p-2 mb-2 h-12">
                                <div className="space-y-0">
                                    <p className="text-sm text-muted-foreground leading-tight">
                                        Fetch directly from the IMAP server
                                    </p>
                                </div>
                                <Switch
                                    checked={remote}
                                    onCheckedChange={(checked) => {
                                        setSelectedMailbox(undefined);
                                        setRemote(checked);
                                        setPage(0);
                                        setSelectedMailbox(undefined);
                                    }}
                                />
                            </div>
                            <div>
                                <AccountSwitcher onAccountSelect={(accountId) => {
                                    localStorage.setItem('mailbox:selectedAccountId', `${accountId}`);
                                    setSelectedAccountId(accountId);
                                    setSelectedMailbox(undefined);
                                    setSelectedIds([]);
                                }} defaultAccountId={lastSelectedAccountId} />
                            </div>
                            <Separator className="mt-2" />
                            {isMailboxesLoading ? (
                                <div className="space-y-2 p-4">
                                    {Array.from({ length: 5 }).map((_, index) => (
                                        <div key={index} className="space-y-2">
                                            <div className="flex items-center space-x-2">
                                                <Skeleton className="h-4 w-4 rounded-full" />
                                                <Skeleton className="h-4 w-[200px]" />
                                            </div>
                                            <div className="pl-6 space-y-2">
                                                {Array.from({ length: 3 }).map((_, subIndex) => (
                                                    <div key={subIndex} className="flex items-center space-x-2">
                                                        <Skeleton className="h-4 w-4 rounded-full" />
                                                        <Skeleton className="h-4 w-[150px]" />
                                                    </div>
                                                ))}
                                            </div>
                                        </div>
                                    ))}
                                </div>
                            ) : (
                                <TreeView
                                    data={buildTree(mailboxes ?? [])}
                                    clickRowToSelect={true}
                                    onSelectChange={(item) => {
                                        if (item) {
                                            setSelectedMailbox(mailboxes?.find(m => m.id === parseInt(item.id, 10)))
                                            setSelectedIds([]);
                                            setPage(0);
                                        } else {
                                            setSelectedMailbox(undefined)
                                        }
                                    }}
                                />
                            )}
                        </ScrollArea>
                    </ResizablePanel>
                    <ResizableHandle withHandle className="h-[calc(100vh-7rem)]" />
                    <ResizablePanel defaultSize={defaultLayout[1]}>
                        {selectedMailbox && <div>
                            <Separator />
                            <div className="flex items-center px-4 py-2">
                                <h2 className="text-xl font-bold cursor-pointer hover:underline" onClick={() => setOpen("mailbox")}>
                                    {selectedMailbox?.name}
                                </h2>
                                {selectedIds.length > 0 && <Dot className="ml-2 text-sm text-muted-foreground" />}
                                {selectedIds.length > 0 && <div className="ml-2 text-sm text-muted-foreground">
                                    {selectedIds.length} {selectedIds.length > 1 ? 'emails' : 'email'} selected
                                </div>}
                            </div>
                            <Separator />
                            <div className="flex flex-wrap items-center gap-2 px-4 py-2">
                                <div className="ml-3 w-10">
                                    {selectedIds.length > 0 ? (
                                        <div
                                            className="peer h-4 w-4 shrink-0 rounded-sm border border-primary shadow focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50 data-[state=checked]:bg-primary data-[state=checked]:text-primary-foreground"
                                            onClick={(e) => {
                                                e.stopPropagation();
                                                setSelectedIds([]);
                                            }}
                                        >
                                            <div className="flex items-center justify-center h-full">
                                                <span className="text-sm font-bold">-</span>
                                            </div>
                                        </div>
                                    ) : (
                                        <div
                                            className="peer h-4 w-4 shrink-0 rounded-sm border border-primary shadow focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50 data-[state=checked]:bg-primary data-[state=checked]:text-primary-foreground"
                                            onClick={(e) => {
                                                e.stopPropagation();
                                                setSelectedIds(envelopes?.items.map((e) => e.id) ?? []);
                                            }}
                                        />
                                    )}
                                </div>
                                <Button variant="outline" className="flex items-center" onClick={() => setOpen('filters')}>
                                    <ListFilterPlus />
                                    Filters
                                </Button>
                                {currentFilter && (
                                    <Button variant="outline" onClick={clearFilter}>
                                        <X />
                                        Clear Filters
                                    </Button>
                                )}
                                {selectedIds.length > 0 && (
                                    <>
                                        <Separator orientation="vertical" className="mx-1 h-6" />
                                        <MoveTo
                                            isMailboxesLoading={isMailboxesLoading}
                                            mailboxes={mailboxes}
                                            accountId={selectedAccountId!}
                                            mailbox={selectedMailbox?.name}
                                            triggerUpdate={triggerUpdate}
                                            selectedIds={selectedIds}
                                            setSelectedIds={setSelectedIds}
                                        />
                                        <Separator orientation="vertical" className="mx-1 h-6" />
                                        <Button variant="outline" className="flex items-center" onClick={() => setOpen('move-to-trash')}>
                                            <Trash2 />
                                            Delete
                                        </Button>
                                        {!isGmailApi && <>
                                            <Separator orientation="vertical" className="mx-1 h-6" />
                                            <Popover>
                                                <PopoverTrigger asChild>
                                                    <Button variant="outline" className="flex items-center">
                                                        <Flag />
                                                        Flag
                                                    </Button>
                                                </PopoverTrigger>
                                                <PopoverContent className="w-96" align="start">
                                                    <CustomFlagInput selectedAccountId={selectedAccountId} selectedMailbox={selectedMailbox} selectedUids={selectedIds.map(id => Number(id))} />
                                                </PopoverContent>
                                            </Popover>
                                        </>}
                                        <Separator orientation="vertical" className="mx-1 h-6" />
                                        <Button variant="outline" className="flex items-center" onClick={handleMarkFolderRead}>
                                            <MailOpen />
                                            Mark as read
                                        </Button>
                                        <Separator orientation="vertical" className="mx-1 h-6" />
                                        <Button variant="outline" className="flex items-center" onClick={handleMarkFolderUnread}>
                                            <MailIcon />
                                            Mark as unread
                                        </Button>
                                    </>
                                )}
                            </div>
                            <Separator />
                            <div className="mt-2">
                                <ScrollArea className='h-[40rem] w-full pr-4 -mr-4 py-1'>
                                    <MailList
                                        isLoading={isMessagesLoading}
                                        items={(envelopes?.items ?? []).sort((a, b) => {
                                            const dateA = a.internal_date;
                                            const dateB = b.internal_date;

                                            if (dateA === undefined && dateB === undefined) return 0;
                                            if (dateA === undefined) return -1;
                                            if (dateB === undefined) return 1;
                                            return dateB - dateA;
                                        })}
                                        setOpen={setOpen}
                                        setDeleteIds={setDeleteIds}
                                        currentEnvelope={selectedEvelope}
                                        setSelectedIds={setSelectedIds}
                                        selectedIds={selectedIds}
                                        onEnvelopeChanged={(envelope) => {
                                            setOpen('display');
                                            setSelectedEvelope(envelope);
                                        }}
                                    />
                                </ScrollArea>
                                {selectedMailbox && <div className="flex justify-center mt-4">
                                    <EnvelopeListPagination
                                        totalItems={envelopes?.total_items ?? 0}
                                        hasNextPage={hasNextPage}
                                        pageIndex={page}
                                        pageSize={pageSize}
                                        setPageIndex={handlePageChange}
                                        setPageSize={handlePageSizeChange}
                                    />
                                </div>}
                            </div>
                        </div>}
                        {!selectedMailbox && <div className="flex h-[750px] shrink-0 items-center justify-center rounded-md border border-dashed">
                            <div className="mx-auto flex max-w-[420px] flex-col items-center justify-center text-center">
                                <img
                                    src={Logo}
                                    className='mb-6 opacity-20 saturate-0 transition-all duration-300 hover:opacity-100 hover:saturate-100'
                                    width={350}
                                    height={350}
                                    alt='RustMailer Logo'
                                />
                            </div>
                        </div>
                        }
                    </ResizablePanel>
                </ResizablePanelGroup>
            </TooltipProvider>
            <MailboxDialog
                key='mailbox-detail'
                open={open === 'mailbox'}
                currentMailbox={selectedMailbox}
                onOpenChange={() => setOpen('mailbox')}
            />
            <MailDisplayDrawer
                key='mail-display'
                open={open === 'display'}
                setOpen={setOpen}
                onOpenChange={() => setOpen('display')}
                setDeleteIds={setDeleteIds}
                currentEnvelope={selectedEvelope}
                currentMailbox={selectedMailbox}
                currentAccountId={selectedAccountId}

            />
            <EnvelopeDeleteDialog
                key='envelope-move-to-trash'
                open={open === 'move-to-trash'}
                deleteIds={deleteIds}
                setDeleteIds={setDeleteIds}
                accountId={selectedAccountId}
                mailbox={selectedMailbox?.name}
                selectedIds={selectedIds}
                onOpenChange={() => setOpen('move-to-trash')}
            />
            <EnvelopeFilterDialog
                open={open === 'filters'}
                key='envelope-filter-dialog'
                handleSearch={handleSearch}
                isSearching={isSearching}
                remote={true}
                currentFilter={currentFilter}
                onOpenChange={() => setOpen('filters')}
            />
        </MailboxProvider >
    )
}
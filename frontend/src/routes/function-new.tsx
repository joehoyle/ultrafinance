import { Form, Link, redirect, useLoaderData } from "react-router-dom";
import { createFunction } from "../api";
import Button from "../components/Button";
import PageHeader from "../components/PageHeader";
import Editor from "@monaco-editor/react";
import * as monaco from 'monaco-editor/esm/vs/editor/editor.api';
import { CreateFunction } from "../../../bindings/CreateFunction";
import { FormEvent, useRef } from "react";
import Input from "../components/forms/Input";

const newFunctionTemplate = `
export const params = {
    accountId: {
        name: 'Account Id',
        type: 'string',
    },
};

export const supportedEvents = [
    'transaction_created',
];

type Transaction = {
    bookingDate: string;
    transactionAmount: string,
    transactionAmountCurrency: string,
    creditorName?: string,
    debtorName?: string,
    remittanceInformation?: string,
    id: number,
}

type Params = {
    accountId: string;
}

export default async function ( params: Params, transaction: Transaction ) {
    // Do something with transaction.
}
`;
export async function action({ request }: { request: Request }) {
	const formData = await request.formData();
	const _function = await createFunction(Object.fromEntries(formData.entries()) as unknown as CreateFunction);
	return redirect(`/functions/${_function.id}`)
}

export default function FunctionNew() {
	const editorRef = useRef<monaco.editor.IStandaloneCodeEditor | null>(null);
	const sourceRef = useRef<HTMLInputElement>(null);
	function handleEditorDidMount(editor: monaco.editor.IStandaloneCodeEditor) {
		editorRef.current = editor;
	}
	function onSubmit(event: FormEvent) {
		if (sourceRef.current && editorRef.current) {
			sourceRef.current.value = editorRef.current.getValue();
		}
	}
	return <>
		<PageHeader><Link to="/accounts">Functions</Link> &rarr; New</PageHeader>
		<main className="flex-grow p-4">
			<Form className="flex flex-col" method="post" action="/functions/new" onSubmit={onSubmit}>
				<label>
					<div className="text-xs text-gray-500 mb-2">Function Name</div>
					<Input name="name" required className="text-sm text-gray-700 border rounded border-gray-200 p-2 outline-none" type="text" />
				</label>
				<div className="flex-1 py-2 bg-[#1e1e1e] mt-4">
					<Editor
						height="70vh"
						defaultLanguage="typescript"
						defaultValue={newFunctionTemplate}
						theme="vs-dark"
						options={{
							minimap: {
								enabled: false,
							}
						}}
						onMount={handleEditorDidMount}
					/>
				</div>
				<input ref={sourceRef} type="hidden" name="source" />
				<div className="flex space-x-2 mt-4">
					<Button>Create Function</Button>
				</div>
			</Form>
		</main>
	</>
}

import { Form, Link, redirect, useLoaderData } from "react-router-dom";
import { createTrigger, getFunctions } from "../api";
import Button from "../components/Button";
import PageHeader from "../components/PageHeader";
import { CreateTrigger } from "../../../bindings/CreateTrigger";
import FormDataToJson from '../utils/FormDataToJson';
import TriggerBuilder from "../components/TriggerBuilder";

export async function action({ request }: { request: Request }) {
	const formData = await request.formData();
	const data = FormDataToJson<CreateTrigger>(formData);
	const trigger = await createTrigger({
		...data,
		function_id: Number( data.function_id),
	});
	return redirect(`/triggers/${ trigger.id }`);
}

export async function loader() {
	return {
		functions: await getFunctions(),
	}
}

export default function TriggerNew() {
	const { functions } = useLoaderData() as Awaited<ReturnType<typeof loader>>;
	return <>
		<PageHeader><Link to="/accounts">Functions</Link> &rarr; New</PageHeader>
		<main className="flex-grow px-10 py-4">
			<Form className="flex flex-col" method="post" action="/triggers/new">
				<TriggerBuilder functions={ functions } />
				<div className="flex space-x-2 mt-4">
					<Button>Create Trigger</Button>
				</div>
			</Form>
		</main>
	</>
}
